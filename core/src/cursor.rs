use std::{
    cell::{Ref, RefCell},
    mem,
    ops::{Deref, DerefMut},
};

use emacs::{defun, Result, Value, Env, GlobalRef, Vector, IntoLisp, FromLisp};
use tree_sitter::{Tree, TreeCursor};

use crate::{
    types::{self, Shared, BytePos},
    node::{RNode, LispUtils},
    lang::Language,
};

emacs::use_symbols! {
    wrong_type_argument
    tree_or_node_p

    _type        => ":type"
    _named_p     => ":named-p"
    _extra_p     => ":extra-p"
    _error_p     => ":error-p"
    _missing_p   => ":missing-p"
    _has_error_p => ":has-error-p"
    _start_byte  => ":start-byte"
    _start_point => ":start-point"
    _end_byte    => ":end-byte"
    _end_point   => ":end-point"
    _range       => ":range"
    _byte_range  => ":byte-range"

    _field       => ":field"
    _depth       => ":depth"
}

// -------------------------------------------------------------------------------------------------

/// Wrapper around `tree_sitter::TreeCursor` that can have 'static lifetime, by keeping a
/// ref-counted reference to the underlying tree.
#[derive(Clone)]
pub struct RCursor {
    tree: Shared<Tree>,
    inner: TreeCursor<'static>,
}

impl_pred!(cursor_p, &RefCell<RCursor>);

pub struct RCursorBorrow<'e> {
    #[allow(unused)]
    reft: Ref<'e, Tree>,
    cursor: &'e TreeCursor<'e>,
}

impl<'e> Deref for RCursorBorrow<'e> {
    type Target = TreeCursor<'e>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.cursor
    }
}

pub struct RCursorBorrowMut<'e> {
    #[allow(unused)]
    reft: Ref<'e, Tree>,
    cursor: &'e mut TreeCursor<'e>,
}

impl<'e> Deref for RCursorBorrowMut<'e> {
    type Target = TreeCursor<'e>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.cursor
    }
}

impl<'e> DerefMut for RCursorBorrowMut<'e> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.cursor
    }
}

impl RCursor {
    pub fn new<'e, F: FnOnce(&'e Tree) -> TreeCursor<'e>>(tree: Shared<Tree>, f: F) -> Self {
        let rtree = unsafe { types::erase_lifetime(&*tree.borrow()) };
        let inner = unsafe { mem::transmute(f(rtree)) };
        Self { tree, inner }
    }

    pub fn clone_tree(&self) -> Shared<Tree> {
        self.tree.clone()
    }

    #[inline]
    pub fn borrow(&self) -> RCursorBorrow {
        let reft = self.tree.borrow();
        let cursor = &self.inner;
        RCursorBorrow { reft, cursor }
    }

    #[inline]
    pub fn borrow_mut<'e>(&'e mut self) -> RCursorBorrowMut {
        let reft: Ref<'e, Tree> = self.tree.borrow();
        // XXX: Explain the safety here.
        let cursor: &'e mut _ = unsafe { mem::transmute(&mut self.inner) };
        RCursorBorrowMut { reft, cursor }
    }
}

pub enum TreeOrNode<'e> {
    Tree(&'e Shared<Tree>),
    Node(&'e RefCell<RNode>),
}

impl<'e> FromLisp<'e> for TreeOrNode<'e> {
    fn from_lisp(value: Value<'e>) -> Result<Self> {
        if let Ok(value) = value.into_rust() {
            return Ok(Self::Tree(value));
        }
        if let Ok(value) = value.into_rust() {
            return Ok(Self::Node(value))
        }
        value.env.signal(wrong_type_argument, (tree_or_node_p, value))
    }
}

impl<'e> TreeOrNode<'e> {
    fn walk(&self) -> RCursor {
        match *self {
            Self::Tree(tree) => RCursor::new(tree.clone(), |tree| tree.walk()),
            Self::Node(node) => {
                let node = node.borrow();
                RCursor::new(node.clone_tree(), |_| node.borrow().walk())
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------

/// Create a new cursor starting from the given TREE-OR-NODE.
///
/// A cursor allows you to walk a syntax tree more efficiently than is possible
/// using `tsc-get-' functions. It is a mutable object that is always on a certain
/// syntax node, and can be moved imperatively to different nodes.
///
/// If a tree is given, the returned cursor starts on its root node.
#[defun(user_ptr)]
fn make_cursor(tree_or_node: TreeOrNode) -> Result<RCursor> {
    Ok(tree_or_node.walk())
}

/// Return CURSOR's current node.
#[defun]
fn current_node(cursor: &RCursor) -> Result<RNode> {
    Ok(RNode::new(cursor.clone_tree(), |_| cursor.borrow().node()))
}

/// Return the field id of CURSOR's current node.
/// Return nil if the current node doesn't have a field.
#[defun]
fn current_field_id(cursor: &RCursor) -> Result<Option<u16>> {
    Ok(cursor.borrow().field_id())
}

/// Return the field associated with CURSOR's current node, as a keyword.
/// Return nil if the current node is not associated with a field.
#[defun]
fn current_field(cursor: &RCursor) -> Result<Option<&'static GlobalRef>> {
    let cursor = cursor.borrow();
    let language: Language = cursor.reft.language().into();
    Ok(cursor.field_id().and_then(|id| language.info().field_name(id)))
}

macro_rules! defun_cursor_walks {
    ($($(#[$meta:meta])* $($lisp_name:literal)? fn $name:ident $( ( $( $param:ident $($into:ident)? : $itype:ty ),* ) )? -> $type:ty)*) => {
        $(
            $(#[$meta])*
            #[defun$((name = $lisp_name))?]
            fn $name(cursor: &mut RCursor, $( $( $param: $itype ),* )? ) -> Result<$type> {
                Ok(cursor.borrow_mut().$name( $( $( $param $(.$into())? ),* )? ))
            }
        )*
    };
}

defun_cursor_walks! {
    /// Move CURSOR to the first child of its current node.
    /// Return t if CURSOR successfully moved, nil if there were no children.
    fn goto_first_child -> bool

    /// Move CURSOR to the parent node of its current node.
    /// Return t if CURSOR successfully moved, nil if it was already on the root node.
    fn goto_parent -> bool

    /// Move CURSOR to the next sibling of its current node.
    /// Return t if CURSOR successfully moved, nil if there was no next sibling node.
    fn goto_next_sibling -> bool

    /// Move CURSOR to the first child that extends beyond the given BYTEPOS.
    /// Return the index of the child node if one was found, nil otherwise.
    "goto-first-child-for-byte" fn goto_first_child_for_byte(bytepos into: BytePos) -> Option<usize>
}

/// Re-initialize CURSOR to start at a different NODE.
#[defun]
fn reset_cursor(cursor: &mut RCursor, node: &RNode) -> Result<()> {
    Ok(cursor.borrow_mut().reset(*node.borrow()))
}

// -------------------------------------------------------------------------------------------------

enum TraversalState {
    Start,
    Down,
    Right,
    Done,
}

use TraversalState::*;

struct DepthFirstIterator {
    cursor: RCursor,
    state: TraversalState,
    depth: usize,
}

// TODO: Provide a function to move backward.
impl DepthFirstIterator {
    fn new(tree_or_node: TreeOrNode) -> Self {
        Self {
            cursor: tree_or_node.walk(),
            state: Start,
            depth: 0,
        }
    }

    #[inline]
    fn item(&self) -> Option<(RNode, usize)> {
        Some((
            RNode::new(self.cursor.clone_tree(),
                       |_| self.cursor.borrow().node()),
            self.depth,
        ))
    }

    fn close(&mut self) {
        self.state = Done;
    }
}

impl Iterator for DepthFirstIterator {
    type Item = (RNode, usize);

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            Start => {
                self.state = Down;
                self.item()
            }
            Down => {
                if self.cursor.borrow_mut().goto_first_child() {
                    self.depth += 1;
                    self.item()
                } else {
                    self.state = Right;
                    self.next()
                }
            }
            Right => {
                if self.cursor.borrow_mut().goto_next_sibling() {
                    self.state = Down;
                    self.item()
                } else if self.cursor.borrow_mut().goto_parent() {
                    self.depth -= 1;
                    self.next()
                } else {
                    self.state = Done;
                    self.next()
                }
            }
            Done => None
        }
    }
}

/// Create a new depth-first iterator from the given TREE-OR-NODE.
/// The traversal is pre-order.
#[defun(user_ptr)]
fn _iter(tree_or_node: TreeOrNode) -> Result<DepthFirstIterator> {
    Ok(DepthFirstIterator::new(tree_or_node))
}

/// Move ITERATOR to the next node.
/// Return t if ITERATOR successfully moved, nil if there was no next node, or if
/// ITERATOR was closed.
#[defun]
fn _iter_next(iterator: &mut DepthFirstIterator) -> Result<bool> {
    Ok(iterator.next().is_some())
}

/// Close ITERATOR.
#[defun]
fn _iter_close(iterator: &mut DepthFirstIterator) -> Result<()> {
    Ok(iterator.close())
}

/// Retrieve properties of the node that ITERATOR is currently on.
///
/// PROPS is a vector of property names to retrieve.
/// OUTPUT is a vector where the properties will be written to.
#[defun]
fn _iter_current_node(iterator: &mut DepthFirstIterator, props: Vector, output: Vector) -> Result<()> {
    let env = output.value().env;
    let cursor = &iterator.cursor;
    let _ = _current_node(cursor, Some(props), Some(output), env)?;
    for (i, prop) in props.into_iter().enumerate() {
        if prop.eq(_depth.bind(env)) {
            output.set(i, iterator.depth)?;
        }
    }
    Ok(())
}

/// Move ITERATOR to the next node, and retrieve its properties.
///
/// This a combination of `tsc--iter-next' and `tsc--iter-current-node'.
#[defun]
fn _iter_next_node(iterator: &mut DepthFirstIterator, props: Vector, output: Vector) -> Result<bool> {
    if iterator.next().is_some() {
        _iter_current_node(iterator, props, output)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Return CURSOR's current node, if PROPS is nil.
///
/// If PROPS is a vector of keywords, this function returns a vector containing the
/// corresponding node properties instead of the node itself. If OUTPUT is also a
/// vector, this function overwrites its contents instead of creating a new vector.
#[defun]
fn _current_node<'e>(cursor: &RCursor, props: Option<Vector<'e>>, output: Option<Vector<'e>>, env: &'e Env) -> Result<Value<'e>> {
    macro_rules! sugar {
        ($prop:ident, $env:ident) => {
            macro_rules! eq {
                ($name:ident) => ($prop.eq($name.bind($env)))
            }
        }
    }
    let node = cursor.borrow().node();
    match props {
        None => RNode::new(cursor.clone_tree(), |_| node).into_lisp(env),
        Some(props) => {
            let result = match output {
                None => env.make_vector(props.len(), ())?,
                Some(output) => output,
            };
            for (i, prop) in props.into_iter().enumerate() {
                sugar!(prop, env);
                if eq!(_type) {
                    result.set(i, node.lisp_type())?;
                } else if eq!(_byte_range) {
                    result.set(i, node.lisp_byte_range(env)?)?;
                } else if eq!(_start_byte) {
                    result.set(i, node.lisp_start_byte())?;
                } else if eq!(_end_byte) {
                    result.set(i, node.lisp_end_byte())?;
                } else if eq!(_field) {
                    result.set(i, current_field(cursor)?)?;
                } else if eq!(_named_p) {
                    result.set(i, node.is_named())?;
                } else if eq!(_extra_p) {
                    result.set(i, node.is_extra())?;
                } else if eq!(_error_p) {
                    result.set(i, node.is_error())?;
                } else if eq!(_missing_p) {
                    result.set(i, node.is_missing())?;
                } else if eq!(_has_error_p) {
                    result.set(i, node.has_error())?;
                } else if eq!(_start_point) {
                    result.set(i, node.lisp_start_point())?;
                } else if eq!(_end_point) {
                    result.set(i, node.lisp_end_point())?;
                } else if eq!(_range) {
                    result.set(i, node.lisp_range())?;
                } else {
                    result.set(i, ())?;
                }
            }
            result.into_lisp(env)
        }
    }
}

/// Actual logic of `tsc-traverse-mapc'. The wrapper is needed because
/// `emacs-module-rs' doesn't currently support optional arguments.
#[defun]
fn _traverse_mapc(func: Value, tree_or_node: TreeOrNode, props: Option<Vector>) -> Result<()> {
    let mut iterator = DepthFirstIterator::new(tree_or_node);
    let env = func.env;
    let output = match props {
        None => None,
        Some(props) => Some(env.make_vector(props.len(), ())?),
    };
    let mut depth_indexes = Vec::with_capacity(1);
    if let Some(props) = props {
        for (i, prop) in props.into_iter().enumerate() {
            if prop.eq(_depth.bind(env)) {
                depth_indexes.push(i)
            }
        }
    }
    // Can't use a for loop because we need to access the cursor to process each item.
    let mut item: Option<(RNode, usize)> = iterator.next();
    while item.is_some() {
        let result = _current_node(&iterator.cursor, props, output, env)?;
        // let (_, depth) = item.unwrap();

        if let Some(output) = output {
            for i in &depth_indexes {
                output.set(*i, iterator.depth)?;
            }
        }

        // Safety: the returned value is unused.
        unsafe { func.call_unprotected([result])?; }

        // // Safety: the returned value is unused.
        // unsafe { func.call_unprotected((result, depth))?; }

        // // 0
        // unsafe { func.call_unprotected([])?; }

        // // 27
        // unsafe { func.call_unprotected((result, depth, func, props))?; }

        // // 13
        // unsafe { func.call_unprotected((result, depth))?; }

        // // 10
        // env.vector((result, depth))?;

        // // 6
        // env.cons(result, depth)?;

        // // 0
        // use emacs::call::IntoLispArgs;
        // (result, depth).into_lisp_args(env)?;

        item = iterator.next();
    }
    // for (_, depth) in iterator {
    //     let result = _current_node(&iterator.cursor.clone(), props, output, env)?;
    //     func.call((result, depth))?;
    // }
    Ok(())
}
