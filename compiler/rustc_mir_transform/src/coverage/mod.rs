pub mod query;

mod counters;
mod graph;
mod spans;

#[cfg(test)]
mod tests;

use self::counters::{BcbCounter, CoverageCounters};
use self::graph::{BasicCoverageBlock, CoverageGraph};
use self::spans::{BcbMapping, BcbMappingKind, CoverageSpans};

use crate::MirPass;

use rustc_middle::hir;
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::mir::coverage::*;
use rustc_middle::mir::{
    self, BasicBlock, BasicBlockData, Coverage, SourceInfo, Statement, StatementKind, Terminator,
    TerminatorKind,
};
use rustc_middle::ty::TyCtxt;
use rustc_span::def_id::LocalDefId;
use rustc_span::source_map::SourceMap;
use rustc_span::{BytePos, Pos, RelativeBytePos, Span, Symbol};

/// Inserts `StatementKind::Coverage` statements that either instrument the binary with injected
/// counters, via intrinsic `llvm.instrprof.increment`, and/or inject metadata used during codegen
/// to construct the coverage map.
pub struct InstrumentCoverage;

impl<'tcx> MirPass<'tcx> for InstrumentCoverage {
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        sess.instrument_coverage()
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, mir_body: &mut mir::Body<'tcx>) {
        let mir_source = mir_body.source;

        // This pass runs after MIR promotion, but before promoted MIR starts to
        // be transformed, so it should never see promoted MIR.
        assert!(mir_source.promoted.is_none());

        let def_id = mir_source.def_id().expect_local();

        if !is_eligible_for_coverage(tcx, def_id) {
            trace!("InstrumentCoverage skipped for {def_id:?} (not eligible)");
            return;
        }

        // An otherwise-eligible function is still skipped if its start block
        // is known to be unreachable.
        match mir_body.basic_blocks[mir::START_BLOCK].terminator().kind {
            TerminatorKind::Unreachable => {
                trace!("InstrumentCoverage skipped for unreachable `START_BLOCK`");
                return;
            }
            _ => {}
        }

        trace!("InstrumentCoverage starting for {def_id:?}");
        Instrumentor::new(tcx, mir_body).inject_counters();
        trace!("InstrumentCoverage done for {def_id:?}");
    }
}

struct Instrumentor<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    mir_body: &'a mut mir::Body<'tcx>,
    hir_info: ExtractedHirInfo,
    basic_coverage_blocks: CoverageGraph,
}

impl<'a, 'tcx> Instrumentor<'a, 'tcx> {
    fn new(tcx: TyCtxt<'tcx>, mir_body: &'a mut mir::Body<'tcx>) -> Self {
        let hir_info = extract_hir_info(tcx, mir_body.source.def_id().expect_local());

        debug!(?hir_info, "instrumenting {:?}", mir_body.source.def_id());

        let basic_coverage_blocks = CoverageGraph::from_mir(mir_body);

        Self { tcx, mir_body, hir_info, basic_coverage_blocks }
    }

    fn inject_counters(&'a mut self) {
        ////////////////////////////////////////////////////
        // Compute coverage spans from the `CoverageGraph`.
        let Some(coverage_spans) = CoverageSpans::generate_coverage_spans(
            self.mir_body,
            &self.hir_info,
            &self.basic_coverage_blocks,
        ) else {
            // No relevant spans were found in MIR, so skip instrumenting this function.
            return;
        };

        ////////////////////////////////////////////////////
        // Create an optimized mix of `Counter`s and `Expression`s for the `CoverageGraph`. Ensure
        // every coverage span has a `Counter` or `Expression` assigned to its `BasicCoverageBlock`
        // and all `Expression` dependencies (operands) are also generated, for any other
        // `BasicCoverageBlock`s not already associated with a coverage span.
        let bcb_has_coverage_spans = |bcb| coverage_spans.bcb_has_coverage_spans(bcb);
        let coverage_counters = CoverageCounters::make_bcb_counters(
            &self.basic_coverage_blocks,
            bcb_has_coverage_spans,
        );

        let mappings = self.create_mappings(&coverage_spans, &coverage_counters);
        if mappings.is_empty() {
            // No spans could be converted into valid mappings, so skip this function.
            debug!("no spans could be converted into valid mappings; skipping");
            return;
        }

        self.inject_coverage_statements(bcb_has_coverage_spans, &coverage_counters);

        self.mir_body.function_coverage_info = Some(Box::new(FunctionCoverageInfo {
            function_source_hash: self.hir_info.function_source_hash,
            num_counters: coverage_counters.num_counters(),
            expressions: coverage_counters.into_expressions(),
            mappings,
        }));
    }

    /// For each coverage span extracted from MIR, create a corresponding
    /// mapping.
    ///
    /// Precondition: All BCBs corresponding to those spans have been given
    /// coverage counters.
    fn create_mappings(
        &self,
        coverage_spans: &CoverageSpans,
        coverage_counters: &CoverageCounters,
    ) -> Vec<Mapping> {
        let source_map = self.tcx.sess.source_map();
        let body_span = self.hir_info.body_span;

        let source_file = source_map.lookup_source_file(body_span.lo());
        use rustc_session::RemapFileNameExt;
        let file_name =
            Symbol::intern(&source_file.name.for_codegen(self.tcx.sess).to_string_lossy());

        let term_for_bcb = |bcb| {
            coverage_counters
                .bcb_counter(bcb)
                .expect("all BCBs with spans were given counters")
                .as_term()
        };

        coverage_spans
            .all_bcb_mappings()
            .filter_map(|&BcbMapping { kind: bcb_mapping_kind, span }| {
                let kind = match bcb_mapping_kind {
                    BcbMappingKind::Code(bcb) => MappingKind::Code(term_for_bcb(bcb)),
                };
                let code_region = make_code_region(source_map, file_name, span, body_span)?;
                Some(Mapping { kind, code_region })
            })
            .collect::<Vec<_>>()
    }

    /// For each BCB node or BCB edge that has an associated coverage counter,
    /// inject any necessary coverage statements into MIR.
    fn inject_coverage_statements(
        &mut self,
        bcb_has_coverage_spans: impl Fn(BasicCoverageBlock) -> bool,
        coverage_counters: &CoverageCounters,
    ) {
        // Process the counters associated with BCB nodes.
        for (bcb, counter_kind) in coverage_counters.bcb_node_counters() {
            let do_inject = match counter_kind {
                // Counter-increment statements always need to be injected.
                BcbCounter::Counter { .. } => true,
                // The only purpose of expression-used statements is to detect
                // when a mapping is unreachable, so we only inject them for
                // expressions with one or more mappings.
                BcbCounter::Expression { .. } => bcb_has_coverage_spans(bcb),
            };
            if do_inject {
                inject_statement(
                    self.mir_body,
                    self.make_mir_coverage_kind(counter_kind),
                    self.basic_coverage_blocks[bcb].leader_bb(),
                );
            }
        }

        // Process the counters associated with BCB edges.
        for (from_bcb, to_bcb, counter_kind) in coverage_counters.bcb_edge_counters() {
            let do_inject = match counter_kind {
                // Counter-increment statements always need to be injected.
                BcbCounter::Counter { .. } => true,
                // BCB-edge expressions never have mappings, so they never need
                // a corresponding statement.
                BcbCounter::Expression { .. } => false,
            };
            if !do_inject {
                continue;
            }

            // We need to inject a coverage statement into a new BB between the
            // last BB of `from_bcb` and the first BB of `to_bcb`.
            let from_bb = self.basic_coverage_blocks[from_bcb].last_bb();
            let to_bb = self.basic_coverage_blocks[to_bcb].leader_bb();

            let new_bb = inject_edge_counter_basic_block(self.mir_body, from_bb, to_bb);
            debug!(
                "Edge {from_bcb:?} (last {from_bb:?}) -> {to_bcb:?} (leader {to_bb:?}) \
                requires a new MIR BasicBlock {new_bb:?} for edge counter {counter_kind:?}",
            );

            // Inject a counter into the newly-created BB.
            inject_statement(self.mir_body, self.make_mir_coverage_kind(counter_kind), new_bb);
        }
    }

    fn make_mir_coverage_kind(&self, counter_kind: &BcbCounter) -> CoverageKind {
        match *counter_kind {
            BcbCounter::Counter { id } => CoverageKind::CounterIncrement { id },
            BcbCounter::Expression { id } => CoverageKind::ExpressionUsed { id },
        }
    }
}

fn inject_edge_counter_basic_block(
    mir_body: &mut mir::Body<'_>,
    from_bb: BasicBlock,
    to_bb: BasicBlock,
) -> BasicBlock {
    let span = mir_body[from_bb].terminator().source_info.span.shrink_to_hi();
    let new_bb = mir_body.basic_blocks_mut().push(BasicBlockData {
        statements: vec![], // counter will be injected here
        terminator: Some(Terminator {
            source_info: SourceInfo::outermost(span),
            kind: TerminatorKind::Goto { target: to_bb },
        }),
        is_cleanup: false,
    });
    let edge_ref = mir_body[from_bb]
        .terminator_mut()
        .successors_mut()
        .find(|successor| **successor == to_bb)
        .expect("from_bb should have a successor for to_bb");
    *edge_ref = new_bb;
    new_bb
}

fn inject_statement(mir_body: &mut mir::Body<'_>, counter_kind: CoverageKind, bb: BasicBlock) {
    debug!("  injecting statement {counter_kind:?} for {bb:?}");
    let data = &mut mir_body[bb];
    let source_info = data.terminator().source_info;
    let statement = Statement {
        source_info,
        kind: StatementKind::Coverage(Box::new(Coverage { kind: counter_kind })),
    };
    data.statements.insert(0, statement);
}

/// Convert the Span into its file name, start line and column, and end line and column.
///
/// Line numbers and column numbers are 1-based. Unlike most column numbers emitted by
/// the compiler, these column numbers are denoted in **bytes**, because that's what
/// LLVM's `llvm-cov` tool expects to see in coverage maps.
///
/// Returns `None` if the conversion failed for some reason. This shouldn't happen,
/// but it's hard to rule out entirely (especially in the presence of complex macros
/// or other expansions), and if it does happen then skipping a span or function is
/// better than an ICE or `llvm-cov` failure that the user might have no way to avoid.
fn make_code_region(
    source_map: &SourceMap,
    file_name: Symbol,
    span: Span,
    body_span: Span,
) -> Option<CodeRegion> {
    debug!(
        "Called make_code_region(file_name={}, span={}, body_span={})",
        file_name,
        source_map.span_to_diagnostic_string(span),
        source_map.span_to_diagnostic_string(body_span)
    );

    let lo = span.lo();
    let hi = span.hi();

    let file = source_map.lookup_source_file(lo);
    if !file.contains(hi) {
        debug!(?span, ?file, ?lo, ?hi, "span crosses multiple files; skipping");
        return None;
    }

    // Column numbers need to be in bytes, so we can't use the more convenient
    // `SourceMap` methods for looking up file coordinates.
    let rpos_and_line_and_byte_column = |pos: BytePos| -> Option<(RelativeBytePos, usize, usize)> {
        let rpos = file.relative_position(pos);
        let line_index = file.lookup_line(rpos)?;
        let line_start = file.lines()[line_index];
        // Line numbers and column numbers are 1-based, so add 1 to each.
        Some((rpos, line_index + 1, (rpos - line_start).to_usize() + 1))
    };

    let (lo_rpos, mut start_line, mut start_col) = rpos_and_line_and_byte_column(lo)?;
    let (hi_rpos, mut end_line, mut end_col) = rpos_and_line_and_byte_column(hi)?;

    // If the span is empty, try to expand it horizontally by one character's
    // worth of bytes, so that it is more visible in `llvm-cov` reports.
    // We do this after resolving line/column numbers, so that empty spans at the
    // end of a line get an extra column instead of wrapping to the next line.
    if span.is_empty()
        && body_span.contains(span)
        && let Some(src) = &file.src
    {
        // Prefer to expand the end position, if it won't go outside the body span.
        if hi < body_span.hi() {
            let hi_rpos = hi_rpos.to_usize();
            let nudge_bytes = src.ceil_char_boundary(hi_rpos + 1) - hi_rpos;
            end_col += nudge_bytes;
        } else if lo > body_span.lo() {
            let lo_rpos = lo_rpos.to_usize();
            let nudge_bytes = lo_rpos - src.floor_char_boundary(lo_rpos - 1);
            // Subtract the nudge, but don't go below column 1.
            start_col = start_col.saturating_sub(nudge_bytes).max(1);
        }
        // If neither nudge could be applied, stick with the empty span coordinates.
    }

    // Apply an offset so that code in doctests has correct line numbers.
    // FIXME(#79417): Currently we have no way to offset doctest _columns_.
    start_line = source_map.doctest_offset_line(&file.name, start_line);
    end_line = source_map.doctest_offset_line(&file.name, end_line);

    check_code_region(CodeRegion {
        file_name,
        start_line: start_line as u32,
        start_col: start_col as u32,
        end_line: end_line as u32,
        end_col: end_col as u32,
    })
}

/// If `llvm-cov` sees a code region that is improperly ordered (end < start),
/// it will immediately exit with a fatal error. To prevent that from happening,
/// discard regions that are improperly ordered, or might be interpreted in a
/// way that makes them improperly ordered.
fn check_code_region(code_region: CodeRegion) -> Option<CodeRegion> {
    let CodeRegion { file_name: _, start_line, start_col, end_line, end_col } = code_region;

    // Line/column coordinates are supposed to be 1-based. If we ever emit
    // coordinates of 0, `llvm-cov` might misinterpret them.
    let all_nonzero = [start_line, start_col, end_line, end_col].into_iter().all(|x| x != 0);
    // Coverage mappings use the high bit of `end_col` to indicate that a
    // region is actually a "gap" region, so make sure it's unset.
    let end_col_has_high_bit_unset = (end_col & (1 << 31)) == 0;
    // If a region is improperly ordered (end < start), `llvm-cov` will exit
    // with a fatal error, which is inconvenient for users and hard to debug.
    let is_ordered = (start_line, start_col) <= (end_line, end_col);

    if all_nonzero && end_col_has_high_bit_unset && is_ordered {
        Some(code_region)
    } else {
        debug!(
            ?code_region,
            ?all_nonzero,
            ?end_col_has_high_bit_unset,
            ?is_ordered,
            "Skipping code region that would be misinterpreted or rejected by LLVM"
        );
        // If this happens in a debug build, ICE to make it easier to notice.
        debug_assert!(false, "Improper code region: {code_region:?}");
        None
    }
}

fn is_eligible_for_coverage(tcx: TyCtxt<'_>, def_id: LocalDefId) -> bool {
    // Only instrument functions, methods, and closures (not constants since they are evaluated
    // at compile time by Miri).
    // FIXME(#73156): Handle source code coverage in const eval, but note, if and when const
    // expressions get coverage spans, we will probably have to "carve out" space for const
    // expressions from coverage spans in enclosing MIR's, like we do for closures. (That might
    // be tricky if const expressions have no corresponding statements in the enclosing MIR.
    // Closures are carved out by their initial `Assign` statement.)
    if !tcx.def_kind(def_id).is_fn_like() {
        trace!("InstrumentCoverage skipped for {def_id:?} (not an fn-like)");
        return false;
    }

    // Don't instrument functions with `#[automatically_derived]` on their
    // enclosing impl block, on the assumption that most users won't care about
    // coverage for derived impls.
    if let Some(impl_of) = tcx.impl_of_method(def_id.to_def_id())
        && tcx.is_automatically_derived(impl_of)
    {
        trace!("InstrumentCoverage skipped for {def_id:?} (automatically derived)");
        return false;
    }

    if tcx.codegen_fn_attrs(def_id).flags.contains(CodegenFnAttrFlags::NO_COVERAGE) {
        trace!("InstrumentCoverage skipped for {def_id:?} (`#[coverage(off)]`)");
        return false;
    }

    true
}

/// Function information extracted from HIR by the coverage instrumentor.
#[derive(Debug)]
struct ExtractedHirInfo {
    function_source_hash: u64,
    is_async_fn: bool,
    fn_sig_span: Span,
    body_span: Span,
}

fn extract_hir_info<'tcx>(tcx: TyCtxt<'tcx>, def_id: LocalDefId) -> ExtractedHirInfo {
    // FIXME(#79625): Consider improving MIR to provide the information needed, to avoid going back
    // to HIR for it.

    let hir_node = tcx.hir_node_by_def_id(def_id);
    let (_, fn_body_id) =
        hir::map::associated_body(hir_node).expect("HIR node is a function with body");
    let hir_body = tcx.hir().body(fn_body_id);

    let is_async_fn = hir_node.fn_sig().is_some_and(|fn_sig| fn_sig.header.is_async());
    let body_span = get_body_span(tcx, hir_body, def_id);

    // The actual signature span is only used if it has the same context and
    // filename as the body, and precedes the body.
    let maybe_fn_sig_span = hir_node.fn_sig().map(|fn_sig| fn_sig.span);
    let fn_sig_span = maybe_fn_sig_span
        .filter(|&fn_sig_span| {
            let source_map = tcx.sess.source_map();
            let file_idx = |span: Span| source_map.lookup_source_file_idx(span.lo());

            fn_sig_span.eq_ctxt(body_span)
                && fn_sig_span.hi() <= body_span.lo()
                && file_idx(fn_sig_span) == file_idx(body_span)
        })
        // If so, extend it to the start of the body span.
        .map(|fn_sig_span| fn_sig_span.with_hi(body_span.lo()))
        // Otherwise, create a dummy signature span at the start of the body.
        .unwrap_or_else(|| body_span.shrink_to_lo());

    let function_source_hash = hash_mir_source(tcx, hir_body);

    ExtractedHirInfo { function_source_hash, is_async_fn, fn_sig_span, body_span }
}

fn get_body_span<'tcx>(
    tcx: TyCtxt<'tcx>,
    hir_body: &rustc_hir::Body<'tcx>,
    def_id: LocalDefId,
) -> Span {
    let mut body_span = hir_body.value.span;

    if tcx.is_closure_or_coroutine(def_id.to_def_id()) {
        // If the current function is a closure, and its "body" span was created
        // by macro expansion or compiler desugaring, try to walk backwards to
        // the pre-expansion call site or body.
        body_span = body_span.source_callsite();
    }

    body_span
}

fn hash_mir_source<'tcx>(tcx: TyCtxt<'tcx>, hir_body: &'tcx rustc_hir::Body<'tcx>) -> u64 {
    // FIXME(cjgillot) Stop hashing HIR manually here.
    let owner = hir_body.id().hir_id.owner;
    tcx.hir_owner_nodes(owner)
        .unwrap()
        .opt_hash_including_bodies
        .unwrap()
        .to_smaller_hash()
        .as_u64()
}
