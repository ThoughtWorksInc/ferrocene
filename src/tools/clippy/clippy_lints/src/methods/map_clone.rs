use clippy_config::msrvs::{self, Msrv};
use clippy_utils::diagnostics::span_lint_and_sugg;
use clippy_utils::source::snippet_with_applicability;
use clippy_utils::ty::{is_copy, is_type_diagnostic_item};
use clippy_utils::{is_diag_trait_item, match_def_path, paths, peel_blocks};
use rustc_errors::Applicability;
use rustc_hir as hir;
use rustc_hir::def_id::DefId;
use rustc_lint::LateContext;
use rustc_middle::mir::Mutability;
use rustc_middle::ty;
use rustc_middle::ty::adjustment::Adjust;
use rustc_span::symbol::Ident;
use rustc_span::{sym, Span};

use super::MAP_CLONE;

// If this `map` is called on an `Option` or a `Result` and the previous call is `as_ref`, we don't
// run this lint because it would overlap with `useless_asref` which provides a better suggestion
// in this case.
fn should_run_lint(cx: &LateContext<'_>, e: &hir::Expr<'_>, method_id: DefId) -> bool {
    if is_diag_trait_item(cx, method_id, sym::Iterator) {
        return true;
    }
    // We check if it's an `Option` or a `Result`.
    if let Some(id) = cx.tcx.impl_of_method(method_id) {
        let identity = cx.tcx.type_of(id).instantiate_identity();
        if !is_type_diagnostic_item(cx, identity, sym::Option) && !is_type_diagnostic_item(cx, identity, sym::Result) {
            return false;
        }
    } else {
        return false;
    }
    // We check if the previous method call is `as_ref`.
    if let hir::ExprKind::MethodCall(path1, receiver, _, _) = &e.kind
        && let hir::ExprKind::MethodCall(path2, _, _, _) = &receiver.kind
    {
        return path2.ident.name != sym::as_ref || path1.ident.name != sym::map;
    }

    true
}

pub(super) fn check(cx: &LateContext<'_>, e: &hir::Expr<'_>, recv: &hir::Expr<'_>, arg: &hir::Expr<'_>, msrv: &Msrv) {
    if let Some(method_id) = cx.typeck_results().type_dependent_def_id(e.hir_id)
        && should_run_lint(cx, e, method_id)
    {
        match arg.kind {
            hir::ExprKind::Closure(&hir::Closure { body, .. }) => {
                let closure_body = cx.tcx.hir().body(body);
                let closure_expr = peel_blocks(closure_body.value);
                match closure_body.params[0].pat.kind {
                    hir::PatKind::Ref(inner, hir::Mutability::Not) => {
                        if let hir::PatKind::Binding(hir::BindingAnnotation::NONE, .., name, None) = inner.kind {
                            if ident_eq(name, closure_expr) {
                                lint_explicit_closure(cx, e.span, recv.span, true, msrv);
                            }
                        }
                    },
                    hir::PatKind::Binding(hir::BindingAnnotation::NONE, .., name, None) => {
                        match closure_expr.kind {
                            hir::ExprKind::Unary(hir::UnOp::Deref, inner) => {
                                if ident_eq(name, inner) {
                                    if let ty::Ref(.., Mutability::Not) = cx.typeck_results().expr_ty(inner).kind() {
                                        lint_explicit_closure(cx, e.span, recv.span, true, msrv);
                                    }
                                }
                            },
                            hir::ExprKind::MethodCall(method, obj, [], _) => {
                                if ident_eq(name, obj) && method.ident.name == sym::clone
                                && let Some(fn_id) = cx.typeck_results().type_dependent_def_id(closure_expr.hir_id)
                                && let Some(trait_id) = cx.tcx.trait_of_item(fn_id)
                                && cx.tcx.lang_items().clone_trait().map_or(false, |id| id == trait_id)
                                // no autoderefs
                                && !cx.typeck_results().expr_adjustments(obj).iter()
                                    .any(|a| matches!(a.kind, Adjust::Deref(Some(..))))
                                {
                                    let obj_ty = cx.typeck_results().expr_ty(obj);
                                    if let ty::Ref(_, ty, mutability) = obj_ty.kind() {
                                        if matches!(mutability, Mutability::Not) {
                                            let copy = is_copy(cx, *ty);
                                            lint_explicit_closure(cx, e.span, recv.span, copy, msrv);
                                        }
                                    } else {
                                        lint_needless_cloning(cx, e.span, recv.span);
                                    }
                                }
                            },
                            hir::ExprKind::Call(call, [_]) => {
                                if let hir::ExprKind::Path(qpath) = call.kind {
                                    handle_path(cx, call, &qpath, e, recv);
                                }
                            },
                            _ => {},
                        }
                    },
                    _ => {},
                }
            },
            hir::ExprKind::Path(qpath) => handle_path(cx, arg, &qpath, e, recv),
            _ => {},
        }
    }
}

fn handle_path(
    cx: &LateContext<'_>,
    arg: &hir::Expr<'_>,
    qpath: &hir::QPath<'_>,
    e: &hir::Expr<'_>,
    recv: &hir::Expr<'_>,
) {
    if let Some(path_def_id) = cx.qpath_res(qpath, arg.hir_id).opt_def_id()
        && match_def_path(cx, path_def_id, &paths::CLONE_TRAIT_METHOD)
    {
        // FIXME: It would be better to infer the type to check if it's copyable or not
        // to suggest to use `.copied()` instead of `.cloned()` where applicable.
        lint_path(cx, e.span, recv.span);
    }
}

fn ident_eq(name: Ident, path: &hir::Expr<'_>) -> bool {
    if let hir::ExprKind::Path(hir::QPath::Resolved(None, path)) = path.kind {
        path.segments.len() == 1 && path.segments[0].ident == name
    } else {
        false
    }
}

fn lint_needless_cloning(cx: &LateContext<'_>, root: Span, receiver: Span) {
    span_lint_and_sugg(
        cx,
        MAP_CLONE,
        root.trim_start(receiver).unwrap(),
        "you are needlessly cloning iterator elements",
        "remove the `map` call",
        String::new(),
        Applicability::MachineApplicable,
    );
}

fn lint_path(cx: &LateContext<'_>, replace: Span, root: Span) {
    let mut applicability = Applicability::MachineApplicable;

    span_lint_and_sugg(
        cx,
        MAP_CLONE,
        replace,
        "you are explicitly cloning with `.map()`",
        "consider calling the dedicated `cloned` method",
        format!(
            "{}.cloned()",
            snippet_with_applicability(cx, root, "..", &mut applicability),
        ),
        applicability,
    );
}

fn lint_explicit_closure(cx: &LateContext<'_>, replace: Span, root: Span, is_copy: bool, msrv: &Msrv) {
    let mut applicability = Applicability::MachineApplicable;

    let (message, sugg_method) = if is_copy && msrv.meets(msrvs::ITERATOR_COPIED) {
        ("you are using an explicit closure for copying elements", "copied")
    } else {
        ("you are using an explicit closure for cloning elements", "cloned")
    };

    span_lint_and_sugg(
        cx,
        MAP_CLONE,
        replace,
        message,
        &format!("consider calling the dedicated `{sugg_method}` method"),
        format!(
            "{}.{sugg_method}()",
            snippet_with_applicability(cx, root, "..", &mut applicability),
        ),
        applicability,
    );
}
