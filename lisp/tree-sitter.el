;;; tree-sitter.el --- Incremental parsing system -*- lexical-binding: t; coding: utf-8 -*-

;; Copyright (C) 2019  Tuấn-Anh Nguyễn
;;
;; Author: Tuấn-Anh Nguyễn <ubolonton@gmail.com>
;; Keywords: languages tools parsers dynamic-modules tree-sitter
;; Homepage: https://github.com/ubolonton/emacs-tree-sitter
;; Version: 0.6.0
;; Package-Requires: ((emacs "25.1"))
;; License: MIT

;;; Commentary:

;; This is an Emacs binding for tree-sitter, an incremental parsing system
;; (https://tree-sitter.github.io/tree-sitter/). It includes both the core APIs,
;; and a minor mode that provides a buffer-local up-to-date syntax tree.

;;; Code:

(require 'tree-sitter-core)
(require 'tree-sitter-load)

(defgroup tree-sitter nil
  "Incremental parsing system."
  :group 'languages)

(defcustom tree-sitter-after-change-functions nil
  "Functions to call each time `tree-sitter-tree' is updated.
Each function will be called with a single argument: the OLD-TREE. This argument
will be nil when the buffer is parsed for the first time.

For initialization logic that should be run only once, use
`tree-sitter-after-first-parse-hook' instead."
  :type 'hook
  :group 'tree-sitter)

(defcustom tree-sitter-after-first-parse-hook nil
  "Functions to call after the buffer is parsed for the first time.
This hook should be used for initialization logic that requires inspecting the
syntax tree. It is run after `tree-sitter-mode-hook'."
  :type 'hook
  :group 'tree-sitter)

(defcustom tree-sitter-after-on-hook nil
  "Functions to call after enabling `tree-sitter-mode'.
Use this to enable other minor modes that depends on the syntax tree."
  :type 'hook
  :group 'tree-sitter)

(defcustom tree-sitter-major-mode-language-alist nil
  "Alist that maps major modes to tree-sitter language names."
  :group 'tree-sitter
  :type '(alist :key-type symbol
                :value-type symbol))

(defvar-local tree-sitter-tree nil
  "Tree-sitter syntax tree.")

(defvar-local tree-sitter-parser nil
  "Tree-sitter parser.")

(defvar-local tree-sitter-language nil
  "Tree-sitter language.")

(defvar-local tree-sitter--text-before-change nil)

(defvar-local tree-sitter--beg-before-change nil)

(defun tree-sitter--before-change (beg old-end)
  "Update relevant editing states. Installed on `before-change-functions'.
BEG and OLD-END are the begin and end positions of the text to be changed."
  (setq tree-sitter--beg-before-change beg)
  (ts--without-restriction
    ;; TODO: Fallback to a full parse if this region is too big.
    (setq tree-sitter--text-before-change
          (buffer-substring-no-properties beg old-end))))

;;; TODO: How do we batch *after* hooks to re-parse only once? Maybe using
;;; `run-with-idle-timer' with 0-second timeout?
;;;
;;; XXX: Figure out how to detect whether it was a text-property-only change.
;;; There's no point in reparsing in these situations.
(defun tree-sitter--after-change (beg new-end old-len)
  "Update relevant editing states and reparse the buffer (incrementally).
Installed on `after-change-functions'.

BEG is the begin position of the change.
NEW-END is the end position of the changed text.
OLD-LEN is the char length of the old text."
  (when tree-sitter-tree
    (let ((beg-byte (position-bytes beg))
          (new-end-byte (position-bytes new-end))
          old-end-byte
          beg-point old-end-point new-end-point)
      (ts--save-context
        (setq beg-point (ts--point-from-position beg)
              new-end-point (ts--point-from-position new-end)))
      ;; Compute the old text's end byte position, line number, byte column.
      ;;
      ;; Tree-sitter works with byte positions, line numbers, byte columns.
      ;; Emacs primarily works with character positions. Converting the latter
      ;; to the former, for the end of the old text, requires looking at the
      ;; actual old text's content. Tree-sitter itself cannot do this, because
      ;; it is designed to keep track of only the numbers, not a mirror of the
      ;; buffer's text. Without re-designing Emac's change tracking mechanism,
      ;; we store the old text through`tree-sitter--before-change', and inspect
      ;; it here. TODO XXX FIX: Improve Emac's change tracking mechanism.
      (if (= old-len 0)
          (setq old-end-byte beg-byte
                old-end-point beg-point)
        (let ((old-text tree-sitter--text-before-change)
              (rel-beg (- beg tree-sitter--beg-before-change)))
          (with-temp-buffer
            (insert old-text)
            (pcase-let*
                ((rel-pos (+ 1 rel-beg old-len))
                 (rel-byte (position-bytes rel-pos))
                 (`(,beg-line-number . ,beg-byte-column) beg-point)
                 (`(,rel-line-number . ,rel-byte-column) (ts--point-from-position rel-pos))
                 (old-end-line-number (+ beg-line-number
                                         rel-line-number -1))
                 (old-end-byte-column (if (> rel-line-number 1)
                                          rel-byte-column
                                        (+ beg-byte-column rel-byte-column))))
              (setq old-end-byte (+ beg-byte rel-byte -1)
                    old-end-point `(,old-end-line-number . ,old-end-byte-column))))))
      (ts-edit-tree tree-sitter-tree
                    beg-byte old-end-byte new-end-byte
                    beg-point old-end-point new-end-point)
      (tree-sitter--do-parse))))

(defun tree-sitter--do-parse ()
  "Parse the current buffer and update the syntax tree."
  (let ((old-tree tree-sitter-tree))
    (setq tree-sitter-tree
          ;; https://github.com/ubolonton/emacs-tree-sitter/issues/3
          (ts--without-restriction
            (ts-parse-chunks tree-sitter-parser #'ts-buffer-input old-tree)))
    (run-hook-with-args 'tree-sitter-after-change-functions old-tree)))

(defun tree-sitter--setup ()
  "Enable `tree-sitter' in the current buffer."
  (unless tree-sitter-language
    ;; Determine the language symbol based on `major-mode' .
    (let ((lang-symbol (alist-get major-mode tree-sitter-major-mode-language-alist)))
      (unless lang-symbol
        (error "No language registered for major mode `%s'" major-mode))
      (setq tree-sitter-language (tree-sitter-require lang-symbol))))
  (unless tree-sitter-parser
    (setq tree-sitter-parser (ts-make-parser))
    (ts-set-language tree-sitter-parser tree-sitter-language))
  (add-hook 'before-change-functions #'tree-sitter--before-change :append :local)
  (add-hook 'after-change-functions #'tree-sitter--after-change :append :local))

(defun tree-sitter--teardown ()
  "Disable `tree-sitter' in the current buffer."
  (remove-hook 'after-change-functions #'tree-sitter--after-change :local)
  (remove-hook 'before-change-functions #'tree-sitter--before-change :local)
  (setq tree-sitter-tree nil
        tree-sitter-parser nil
        tree-sitter-language nil))

(defmacro tree-sitter--error-protect (body-form &rest error-forms)
  "Execute BODY-FORM with ERROR-FORMS as cleanup code that is executed on error.
Unlike `unwind-protect', ERROR-FORMS is not executed if BODY-FORM does not
signal an error."
  (declare (indent 1))
  `(let ((err t))
     (unwind-protect
         (prog1 ,body-form
           (setq err nil))
       (when err
         ,@error-forms))))

;;; TODO: Support the use case where a temporary buffer is created just to
;;; fontify some text. That's what `org-mode' and `markdown-mode' does. Ideally
;;; though, in the long run, they should create multiple buffer-local parsers on
;;; their own, one for each language with code blocks in the file.
;;;###autoload
(define-minor-mode tree-sitter-mode
  "Minor mode that keeps an up-to-date syntax tree using incremental parsing."
  :init-value nil
  :lighter "tree-sitter"
  :after-hook (when tree-sitter-mode
                (unless tree-sitter-tree
                  (tree-sitter--do-parse)
                  (run-hooks 'tree-sitter-after-first-parse-hook)))
  (if tree-sitter-mode
      (tree-sitter--error-protect
          (progn (tree-sitter--setup)
                 (run-hooks 'tree-sitter-after-on-hook))
        (setq tree-sitter-mode nil)
        (tree-sitter--teardown))
    (run-hooks 'tree-sitter--before-off-hook)
    (tree-sitter--teardown)))

;;;###autoload
(defun turn-on-tree-sitter-mode ()
  "Turn on `tree-sitter-mode' in a buffer, if possible."
  ;; FIX: Ignore only known errors. Log the rest, at least.
  (ignore-errors
    (tree-sitter-mode 1)))

;;;###autoload
(define-globalized-minor-mode global-tree-sitter-mode
  tree-sitter-mode turn-on-tree-sitter-mode
  :init-value nil
  :group 'tree-sitter)

(defun tree-sitter--funcall-form (func)
  "Return an equivalent to (funcall FUNC) that can be used in a macro.
If FUNC is a quoted symbol, skip the `funcall' indirection."
  (if (and (consp func)
           (memq (car func) '(quote function))
           (symbolp (cadr func)))
      `(,(cadr func))
    `(funcall ,func)))

(defmacro tree-sitter--handle-dependent (mode setup-function teardown-function)
  "Build the block of code that handles the enabling/disabling of a dependent mode.
Use this as the body of the `define-minor-mode' block that defines MODE.

When MODE is enabled, it automatically enables `tree-sitter-mode'. When MODE is
disabled, it does not disable `tree-sitter-mode', since the latter may have been
requested by end user, or other dependent modes.

When `tree-sitter-mode' is disabled, it automatically disables MODE, which will
not function correctly otherwise. This happens before `tree-sitter-mode' cleans
up its own state.

SETUP-FUNCTION is called when MODE is enabled, after MODE variable has been set
to t, and after `tree-sitter-mode' has already been enabled. However, it must
not assume that `tree-sitter-tree' is non-nil, since the first parse may not
happen yet. It should instead set up hooks to handle parse events.

TEARDOWN-FUNCTION is called when MODE is disabled, after MODE variable has been
set to nil. It should clean up any state set up by MODE, and should not signal
any error. It is also called when SETUP-FUNCTION signals an error, to undo any
partial setup.

Both SETUP-FUNCTION and TEARDOWN-FUNCTION should be idempotent."
  (declare (indent 1))
  (let ((setup (tree-sitter--funcall-form setup-function))
        (teardown (tree-sitter--funcall-form teardown-function)))
    `(if ,mode
         (progn
           (tree-sitter--error-protect
               ;; Make sure `tree-sitter-mode' is enabled before MODE.
               (progn
                 (unless tree-sitter-mode
                   (tree-sitter-mode))
                 ,setup)
             ;; Setup failed. Clean things up, leave no trace.
             (setq ,mode nil)
             ,teardown)
           ;; Disable MODE when `tree-sitter-mode' is disabled. Quoting is
           ;; important, because we don't want a variable-capturing closure.
           (add-hook 'tree-sitter--before-off-hook
                     '(lambda () (,mode -1))
                     nil :local))
       ,teardown)))

;;;###autoload
(defun tree-sitter-node-at-point ()
  "Return the syntax node at point."
  (let ((root (ts-root-node tree-sitter-tree))
        (p (point)))
    (ts-get-descendant-for-position-range root p p)))

(provide 'tree-sitter)
;;; tree-sitter.el ends here
