use rustc_ast as ast;
use rustc_ast::visit::{self, AssocCtxt, FnCtxt, FnKind, Visitor};
use rustc_ast::{attr, AssocConstraint, AssocConstraintKind, NodeId};
use rustc_ast::{PatKind, RangeEnd};
use rustc_feature::{AttributeGate, BuiltinAttribute, Features, GateIssue, BUILTIN_ATTRIBUTE_MAP};
use rustc_session::parse::{feature_err, feature_err_issue, feature_warn};
use rustc_session::Session;
use rustc_span::source_map::Spanned;
use rustc_span::symbol::sym;
use rustc_span::Span;
use rustc_target::spec::abi;
use thin_vec::ThinVec;

use crate::errors;

/// The common case.
macro_rules! gate {
    ($visitor:expr, $feature:ident, $span:expr, $explain:expr) => {{
        if !$visitor.features.$feature && !$span.allows_unstable(sym::$feature) {
            feature_err(&$visitor.sess.parse_sess, sym::$feature, $span, $explain).emit();
        }
    }};
    ($visitor:expr, $feature:ident, $span:expr, $explain:expr, $help:expr) => {{
        if !$visitor.features.$feature && !$span.allows_unstable(sym::$feature) {
            feature_err(&$visitor.sess.parse_sess, sym::$feature, $span, $explain)
                .help($help)
                .emit();
        }
    }};
}

/// The unusual case, where the `has_feature` condition is non-standard.
macro_rules! gate_alt {
    ($visitor:expr, $has_feature:expr, $name:expr, $span:expr, $explain:expr) => {{
        if !$has_feature && !$span.allows_unstable($name) {
            feature_err(&$visitor.sess.parse_sess, $name, $span, $explain).emit();
        }
    }};
}

/// The case involving a multispan.
macro_rules! gate_multi {
    ($visitor:expr, $feature:ident, $spans:expr, $explain:expr) => {{
        if !$visitor.features.$feature {
            let spans: Vec<_> =
                $spans.filter(|span| !span.allows_unstable(sym::$feature)).collect();
            if !spans.is_empty() {
                feature_err(&$visitor.sess.parse_sess, sym::$feature, spans, $explain).emit();
            }
        }
    }};
}

/// The legacy case.
macro_rules! gate_legacy {
    ($visitor:expr, $feature:ident, $span:expr, $explain:expr) => {{
        if !$visitor.features.$feature && !$span.allows_unstable(sym::$feature) {
            feature_warn(&$visitor.sess.parse_sess, sym::$feature, $span, $explain);
        }
    }};
}

pub fn check_attribute(attr: &ast::Attribute, sess: &Session, features: &Features) {
    PostExpansionVisitor { sess, features }.visit_attribute(attr)
}

struct PostExpansionVisitor<'a> {
    sess: &'a Session,

    // `sess` contains a `Features`, but this might not be that one.
    features: &'a Features,
}

impl<'a> PostExpansionVisitor<'a> {
    fn check_abi(&self, abi: ast::StrLit, constness: ast::Const) {
        let ast::StrLit { symbol_unescaped, span, .. } = abi;

        if let ast::Const::Yes(_) = constness {
            match symbol_unescaped {
                // Stable
                sym::Rust | sym::C => {}
                abi => gate!(
                    &self,
                    const_extern_fn,
                    span,
                    format!("`{}` as a `const fn` ABI is unstable", abi)
                ),
            }
        }

        match abi::is_enabled(&self.features, span, symbol_unescaped.as_str()) {
            Ok(()) => (),
            Err(abi::AbiDisabled::Unstable { feature, explain }) => {
                feature_err_issue(
                    &self.sess.parse_sess,
                    feature,
                    span,
                    GateIssue::Language,
                    explain,
                )
                .emit();
            }
            Err(abi::AbiDisabled::Unrecognized) => {
                if self.sess.opts.pretty.map_or(true, |ppm| ppm.needs_hir()) {
                    self.sess.parse_sess.span_diagnostic.delay_span_bug(
                        span,
                        format!(
                            "unrecognized ABI not caught in lowering: {}",
                            symbol_unescaped.as_str()
                        ),
                    );
                }
            }
        }
    }

    fn check_extern(&self, ext: ast::Extern, constness: ast::Const) {
        if let ast::Extern::Explicit(abi, _) = ext {
            self.check_abi(abi, constness);
        }
    }

    /// Feature gate `impl Trait` inside `type Alias = $type_expr;`.
    fn check_impl_trait(&self, ty: &ast::Ty, in_associated_ty: bool) {
        struct ImplTraitVisitor<'a> {
            vis: &'a PostExpansionVisitor<'a>,
            in_associated_ty: bool,
        }
        impl Visitor<'_> for ImplTraitVisitor<'_> {
            fn visit_ty(&mut self, ty: &ast::Ty) {
                if let ast::TyKind::ImplTrait(..) = ty.kind {
                    if self.in_associated_ty {
                        gate!(
                            &self.vis,
                            impl_trait_in_assoc_type,
                            ty.span,
                            "`impl Trait` in associated types is unstable"
                        );
                    } else {
                        gate!(
                            &self.vis,
                            type_alias_impl_trait,
                            ty.span,
                            "`impl Trait` in type aliases is unstable"
                        );
                    }
                }
                visit::walk_ty(self, ty);
            }
        }
        ImplTraitVisitor { vis: self, in_associated_ty }.visit_ty(ty);
    }

    fn check_late_bound_lifetime_defs(&self, params: &[ast::GenericParam]) {
        // Check only lifetime parameters are present and that the lifetime
        // parameters that are present have no bounds.
        let non_lt_param_spans = params.iter().filter_map(|param| match param.kind {
            ast::GenericParamKind::Lifetime { .. } => None,
            _ => Some(param.ident.span),
        });
        gate_multi!(
            &self,
            non_lifetime_binders,
            non_lt_param_spans,
            crate::fluent_generated::ast_passes_forbidden_non_lifetime_param
        );
        for param in params {
            if !param.bounds.is_empty() {
                let spans: Vec<_> = param.bounds.iter().map(|b| b.span()).collect();
                self.sess.emit_err(errors::ForbiddenLifetimeBound { spans });
            }
        }
    }
}

impl<'a> Visitor<'a> for PostExpansionVisitor<'a> {
    fn visit_attribute(&mut self, attr: &ast::Attribute) {
        let attr_info = attr.ident().and_then(|ident| BUILTIN_ATTRIBUTE_MAP.get(&ident.name));
        // Check feature gates for built-in attributes.
        if let Some(BuiltinAttribute {
            gate: AttributeGate::Gated(_, name, descr, has_feature),
            ..
        }) = attr_info
        {
            gate_alt!(self, has_feature(&self.features), *name, attr.span, *descr);
        }
        // Check unstable flavors of the `#[doc]` attribute.
        if attr.has_name(sym::doc) {
            for nested_meta in attr.meta_item_list().unwrap_or_default() {
                macro_rules! gate_doc { ($($s:literal { $($name:ident => $feature:ident)* })*) => {
                    $($(if nested_meta.has_name(sym::$name) {
                        let msg = concat!("`#[doc(", stringify!($name), ")]` is ", $s);
                        gate!(self, $feature, attr.span, msg);
                    })*)*
                }}

                gate_doc!(
                    "experimental" {
                        cfg => doc_cfg
                        cfg_hide => doc_cfg_hide
                        masked => doc_masked
                        notable_trait => doc_notable_trait
                    }
                    "meant for internal use only" {
                        keyword => rustdoc_internals
                        fake_variadic => rustdoc_internals
                    }
                );
            }
        }
        if !attr.is_doc_comment()
            && let [seg, _] = attr.get_normal_item().path.segments.as_slice()
            && seg.ident.name == sym::diagnostic
            && !self.features.diagnostic_namespace
        {
            let msg = "`#[diagnostic]` attribute name space is experimental";
            gate!(self, diagnostic_namespace, seg.ident.span, msg);
        }

        // Emit errors for non-staged-api crates.
        if !self.features.staged_api {
            if attr.has_name(sym::unstable)
                || attr.has_name(sym::stable)
                || attr.has_name(sym::rustc_const_unstable)
                || attr.has_name(sym::rustc_const_stable)
                || attr.has_name(sym::rustc_default_body_unstable)
            {
                self.sess.emit_err(errors::StabilityOutsideStd { span: attr.span });
            }
        }
    }

    fn visit_item(&mut self, i: &'a ast::Item) {
        match &i.kind {
            ast::ItemKind::ForeignMod(foreign_module) => {
                if let Some(abi) = foreign_module.abi {
                    self.check_abi(abi, ast::Const::No);
                }
            }

            ast::ItemKind::Fn(..) => {
                if attr::contains_name(&i.attrs, sym::start) {
                    gate!(
                        &self,
                        start,
                        i.span,
                        "`#[start]` functions are experimental and their signature may change \
                         over time"
                    );
                }
            }

            ast::ItemKind::Struct(..) => {
                for attr in attr::filter_by_name(&i.attrs, sym::repr) {
                    for item in attr.meta_item_list().unwrap_or_else(ThinVec::new) {
                        if item.has_name(sym::simd) {
                            gate!(
                                &self,
                                repr_simd,
                                attr.span,
                                "SIMD types are experimental and possibly buggy"
                            );
                        }
                    }
                }
            }

            ast::ItemKind::Impl(box ast::Impl { polarity, defaultness, of_trait, .. }) => {
                if let &ast::ImplPolarity::Negative(span) = polarity {
                    gate!(
                        &self,
                        negative_impls,
                        span.to(of_trait.as_ref().map_or(span, |t| t.path.span)),
                        "negative trait bounds are not yet fully implemented; \
                         use marker types for now"
                    );
                }

                if let ast::Defaultness::Default(_) = defaultness {
                    gate!(&self, specialization, i.span, "specialization is unstable");
                }
            }

            ast::ItemKind::Trait(box ast::Trait { is_auto: ast::IsAuto::Yes, .. }) => {
                gate!(
                    &self,
                    auto_traits,
                    i.span,
                    "auto traits are experimental and possibly buggy"
                );
            }

            ast::ItemKind::TraitAlias(..) => {
                gate!(&self, trait_alias, i.span, "trait aliases are experimental");
            }

            ast::ItemKind::MacroDef(ast::MacroDef { macro_rules: false, .. }) => {
                let msg = "`macro` is experimental";
                gate!(&self, decl_macro, i.span, msg);
            }

            ast::ItemKind::TyAlias(box ast::TyAlias { ty: Some(ty), .. }) => {
                self.check_impl_trait(&ty, false)
            }

            _ => {}
        }

        visit::walk_item(self, i);
    }

    fn visit_foreign_item(&mut self, i: &'a ast::ForeignItem) {
        match i.kind {
            ast::ForeignItemKind::Fn(..) | ast::ForeignItemKind::Static(..) => {
                let link_name = attr::first_attr_value_str_by_name(&i.attrs, sym::link_name);
                let links_to_llvm = link_name.is_some_and(|val| val.as_str().starts_with("llvm."));
                if links_to_llvm {
                    gate!(
                        &self,
                        link_llvm_intrinsics,
                        i.span,
                        "linking to LLVM intrinsics is experimental"
                    );
                }
            }
            ast::ForeignItemKind::TyAlias(..) => {
                gate!(&self, extern_types, i.span, "extern types are experimental");
            }
            ast::ForeignItemKind::MacCall(..) => {}
        }

        visit::walk_foreign_item(self, i)
    }

    fn visit_ty(&mut self, ty: &'a ast::Ty) {
        match &ty.kind {
            ast::TyKind::BareFn(bare_fn_ty) => {
                // Function pointers cannot be `const`
                self.check_extern(bare_fn_ty.ext, ast::Const::No);
                self.check_late_bound_lifetime_defs(&bare_fn_ty.generic_params);
            }
            ast::TyKind::Never => {
                gate!(&self, never_type, ty.span, "the `!` type is experimental");
            }
            _ => {}
        }
        visit::walk_ty(self, ty)
    }

    fn visit_generics(&mut self, g: &'a ast::Generics) {
        for predicate in &g.where_clause.predicates {
            match predicate {
                ast::WherePredicate::BoundPredicate(bound_pred) => {
                    // A type binding, eg `for<'c> Foo: Send+Clone+'c`
                    self.check_late_bound_lifetime_defs(&bound_pred.bound_generic_params);
                }
                _ => {}
            }
        }
        visit::walk_generics(self, g);
    }

    fn visit_fn_ret_ty(&mut self, ret_ty: &'a ast::FnRetTy) {
        if let ast::FnRetTy::Ty(output_ty) = ret_ty {
            if let ast::TyKind::Never = output_ty.kind {
                // Do nothing.
            } else {
                self.visit_ty(output_ty)
            }
        }
    }

    fn visit_expr(&mut self, e: &'a ast::Expr) {
        match e.kind {
            ast::ExprKind::TryBlock(_) => {
                gate!(&self, try_blocks, e.span, "`try` expression is experimental");
            }
            _ => {}
        }
        visit::walk_expr(self, e)
    }

    fn visit_pat(&mut self, pattern: &'a ast::Pat) {
        match &pattern.kind {
            PatKind::Slice(pats) => {
                for pat in pats {
                    let inner_pat = match &pat.kind {
                        PatKind::Ident(.., Some(pat)) => pat,
                        _ => pat,
                    };
                    if let PatKind::Range(Some(_), None, Spanned { .. }) = inner_pat.kind {
                        gate!(
                            &self,
                            half_open_range_patterns_in_slices,
                            pat.span,
                            "`X..` patterns in slices are experimental"
                        );
                    }
                }
            }
            PatKind::Box(..) => {
                gate!(&self, box_patterns, pattern.span, "box pattern syntax is experimental");
            }
            PatKind::Range(_, Some(_), Spanned { node: RangeEnd::Excluded, .. }) => {
                gate!(
                    &self,
                    exclusive_range_pattern,
                    pattern.span,
                    "exclusive range pattern syntax is experimental"
                );
            }
            _ => {}
        }
        visit::walk_pat(self, pattern)
    }

    fn visit_poly_trait_ref(&mut self, t: &'a ast::PolyTraitRef) {
        self.check_late_bound_lifetime_defs(&t.bound_generic_params);
        visit::walk_poly_trait_ref(self, t);
    }

    fn visit_fn(&mut self, fn_kind: FnKind<'a>, span: Span, _: NodeId) {
        if let Some(header) = fn_kind.header() {
            // Stability of const fn methods are covered in `visit_assoc_item` below.
            self.check_extern(header.ext, header.constness);
        }

        if let FnKind::Closure(ast::ClosureBinder::For { generic_params, .. }, ..) = fn_kind {
            self.check_late_bound_lifetime_defs(generic_params);
        }

        if fn_kind.ctxt() != Some(FnCtxt::Foreign) && fn_kind.decl().c_variadic() {
            gate!(&self, c_variadic, span, "C-variadic functions are unstable");
        }

        visit::walk_fn(self, fn_kind)
    }

    fn visit_assoc_constraint(&mut self, constraint: &'a AssocConstraint) {
        if let AssocConstraintKind::Bound { .. } = constraint.kind {
            if let Some(ast::GenericArgs::Parenthesized(args)) = constraint.gen_args.as_ref()
                && args.inputs.is_empty()
                && matches!(args.output, ast::FnRetTy::Default(..))
            {
                gate!(
                    &self,
                    return_type_notation,
                    constraint.span,
                    "return type notation is experimental"
                );
            } else {
                gate!(
                    &self,
                    associated_type_bounds,
                    constraint.span,
                    "associated type bounds are unstable"
                );
            }
        }
        visit::walk_assoc_constraint(self, constraint)
    }

    fn visit_assoc_item(&mut self, i: &'a ast::AssocItem, ctxt: AssocCtxt) {
        let is_fn = match &i.kind {
            ast::AssocItemKind::Fn(_) => true,
            ast::AssocItemKind::Type(box ast::TyAlias { ty, .. }) => {
                if let (Some(_), AssocCtxt::Trait) = (ty, ctxt) {
                    gate!(
                        &self,
                        associated_type_defaults,
                        i.span,
                        "associated type defaults are unstable"
                    );
                }
                if let Some(ty) = ty {
                    self.check_impl_trait(ty, true);
                }
                false
            }
            _ => false,
        };
        if let ast::Defaultness::Default(_) = i.kind.defaultness() {
            // Limit `min_specialization` to only specializing functions.
            gate_alt!(
                &self,
                self.features.specialization || (is_fn && self.features.min_specialization),
                sym::specialization,
                i.span,
                "specialization is unstable"
            );
        }
        visit::walk_assoc_item(self, i, ctxt)
    }
}

pub fn check_crate(krate: &ast::Crate, sess: &Session, features: &Features) {
    maybe_stage_features(sess, features, krate);
    check_incompatible_features(sess, features);
    let mut visitor = PostExpansionVisitor { sess, features };

    let spans = sess.parse_sess.gated_spans.spans.borrow();
    macro_rules! gate_all {
        ($gate:ident, $msg:literal) => {
            if let Some(spans) = spans.get(&sym::$gate) {
                for span in spans {
                    gate!(&visitor, $gate, *span, $msg);
                }
            }
        };
        ($gate:ident, $msg:literal, $help:literal) => {
            if let Some(spans) = spans.get(&sym::$gate) {
                for span in spans {
                    gate!(&visitor, $gate, *span, $msg, $help);
                }
            }
        };
    }
    gate_all!(c_str_literals, "`c\"..\"` literals are experimental");
    gate_all!(
        if_let_guard,
        "`if let` guards are experimental",
        "you can write `if matches!(<expr>, <pattern>)` instead of `if let <pattern> = <expr>`"
    );
    gate_all!(let_chains, "`let` expressions in this position are unstable");
    gate_all!(
        async_closure,
        "async closures are unstable",
        "to use an async block, remove the `||`: `async {`"
    );
    gate_all!(
        closure_lifetime_binder,
        "`for<...>` binders for closures are experimental",
        "consider removing `for<...>`"
    );
    gate_all!(more_qualified_paths, "usage of qualified paths in this context is experimental");
    for &span in spans.get(&sym::yield_expr).iter().copied().flatten() {
        if !span.at_least_rust_2024() {
            gate!(&visitor, coroutines, span, "yield syntax is experimental");
        }
    }
    gate_all!(gen_blocks, "gen blocks are experimental");
    gate_all!(raw_ref_op, "raw address of syntax is experimental");
    gate_all!(const_trait_impl, "const trait impls are experimental");
    gate_all!(
        half_open_range_patterns_in_slices,
        "half-open range patterns in slices are unstable"
    );
    gate_all!(inline_const, "inline-const is experimental");
    gate_all!(inline_const_pat, "inline-const in pattern position is experimental");
    gate_all!(associated_const_equality, "associated const equality is incomplete");
    gate_all!(yeet_expr, "`do yeet` expression is experimental");
    gate_all!(dyn_star, "`dyn*` trait objects are experimental");
    gate_all!(const_closures, "const closures are experimental");
    gate_all!(builtin_syntax, "`builtin #` syntax is unstable");
    gate_all!(explicit_tail_calls, "`become` expression is experimental");
    gate_all!(generic_const_items, "generic const items are experimental");
    gate_all!(unnamed_fields, "unnamed fields are not yet fully implemented");

    if !visitor.features.negative_bounds {
        for &span in spans.get(&sym::negative_bounds).iter().copied().flatten() {
            sess.emit_err(errors::NegativeBoundUnsupported { span });
        }
    }

    // All uses of `gate_all_legacy_dont_use!` below this point were added in #65742,
    // and subsequently disabled (with the non-early gating readded).
    // We emit an early future-incompatible warning for these.
    // New syntax gates should go above here to get a hard error gate.
    macro_rules! gate_all_legacy_dont_use {
        ($gate:ident, $msg:literal) => {
            for span in spans.get(&sym::$gate).unwrap_or(&vec![]) {
                gate_legacy!(&visitor, $gate, *span, $msg);
            }
        };
    }

    gate_all_legacy_dont_use!(trait_alias, "trait aliases are experimental");
    gate_all_legacy_dont_use!(associated_type_bounds, "associated type bounds are unstable");
    // Despite being a new feature, `where T: Trait<Assoc(): Sized>`, which is RTN syntax now,
    // used to be gated under associated_type_bounds, which are right above, so RTN needs to
    // be too.
    gate_all_legacy_dont_use!(return_type_notation, "return type notation is experimental");
    gate_all_legacy_dont_use!(decl_macro, "`macro` is experimental");
    gate_all_legacy_dont_use!(box_patterns, "box pattern syntax is experimental");
    gate_all_legacy_dont_use!(
        exclusive_range_pattern,
        "exclusive range pattern syntax is experimental"
    );
    gate_all_legacy_dont_use!(try_blocks, "`try` blocks are unstable");
    gate_all_legacy_dont_use!(auto_traits, "`auto` traits are unstable");

    visit::walk_crate(&mut visitor, krate);
}

fn maybe_stage_features(sess: &Session, features: &Features, krate: &ast::Crate) {
    // checks if `#![feature]` has been used to enable any lang feature
    // does not check the same for lib features unless there's at least one
    // declared lang feature
    if !sess.opts.unstable_features.is_nightly_build() {
        let lang_features = &features.declared_lang_features;
        if lang_features.len() == 0 {
            return;
        }
        for attr in krate.attrs.iter().filter(|attr| attr.has_name(sym::feature)) {
            let mut err = errors::FeatureOnNonNightly {
                span: attr.span,
                channel: option_env!("CFG_RELEASE_CHANNEL").unwrap_or("(unknown)"),
                stable_features: vec![],
                sugg: None,
            };

            let mut all_stable = true;
            for ident in
                attr.meta_item_list().into_iter().flatten().flat_map(|nested| nested.ident())
            {
                let name = ident.name;
                let stable_since = lang_features
                    .iter()
                    .flat_map(|&(feature, _, since)| if feature == name { since } else { None })
                    .next();
                if let Some(since) = stable_since {
                    err.stable_features.push(errors::StableFeature { name, since });
                } else {
                    all_stable = false;
                }
            }
            if all_stable {
                err.sugg = Some(attr.span);
            }
            sess.parse_sess.span_diagnostic.emit_err(err);
        }
    }
}

fn check_incompatible_features(sess: &Session, features: &Features) {
    let declared_features = features
        .declared_lang_features
        .iter()
        .copied()
        .map(|(name, span, _)| (name, span))
        .chain(features.declared_lib_features.iter().copied());

    for (f1, f2) in rustc_feature::INCOMPATIBLE_FEATURES
        .iter()
        .filter(|&&(f1, f2)| features.active(f1) && features.active(f2))
    {
        if let Some((f1_name, f1_span)) = declared_features.clone().find(|(name, _)| name == f1) {
            if let Some((f2_name, f2_span)) = declared_features.clone().find(|(name, _)| name == f2)
            {
                let spans = vec![f1_span, f2_span];
                sess.emit_err(errors::IncompatibleFeatures { spans, f1: f1_name, f2: f2_name });
            }
        }
    }
}
