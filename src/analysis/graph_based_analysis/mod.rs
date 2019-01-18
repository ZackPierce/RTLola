pub mod dependency_graph;
pub mod evaluation_order;
pub mod future_dependency;
pub mod space_requirements;

use super::lola_version::LolaVersionTable;
use super::typing::TypeTable;
use super::DeclarationTable;
use crate::ast::LolaSpec;
use crate::reporting::Handler;
use ast_node::NodeId;
use ast_node::Span;
use petgraph::Directed;
use petgraph::Graph;
use std::time::Duration;

pub(crate) use self::evaluation_order::EvaluationOrderResult;
pub(crate) use self::future_dependency::FutureDependentStreams;
pub(crate) use self::space_requirements::SpaceRequirements;
use self::space_requirements::TrackingRequirements;

pub(crate) struct GraphAnalysisResult {
    pub(crate) dependency_graph: DependencyGraph,
    pub(crate) evaluation_order: EvaluationOrderResult,
    pub(crate) future_dependent_streams: FutureDependentStreams,
    pub(crate) space_requirements: SpaceRequirements,
    pub(crate) tracking_requirements: TrackingRequirements,
}

pub(crate) fn analyze<'a>(
    spec: &'a LolaSpec,
    version_analysis: &LolaVersionTable,
    declaration_table: &DeclarationTable<'a>,
    type_table: &TypeTable,
    handler: &Handler,
) -> Option<GraphAnalysisResult> {
    let dependency_analysis =
        dependency_graph::analyse_dependencies(spec, version_analysis, declaration_table, &handler);

    if handler.contains_error() {
        handler.error("aborting due to previous error");
        return None;
    }

    let (evaluation_order_result, pruned_graph) =
        evaluation_order::determine_evaluation_order(dependency_analysis.dependency_graph);

    let future_dependent_streams = future_dependency::future_dependent_stream(&pruned_graph);

    let space_requirements =
        space_requirements::determine_buffer_size(&pruned_graph, &future_dependent_streams);

    let tracking_requirements = space_requirements::determine_tracking_size(
        &pruned_graph,
        type_table,
        &future_dependent_streams,
    );

    Some(GraphAnalysisResult {
        dependency_graph: pruned_graph,
        evaluation_order: evaluation_order_result,
        future_dependent_streams,
        space_requirements,
        tracking_requirements,
    })
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum StreamNode {
    ClassicInput(ast_node::NodeId),
    ClassicOutput(ast_node::NodeId),
    ParameterizedInput(ast_node::NodeId),
    ParameterizedOutput(ast_node::NodeId),
    RTOutput(ast_node::NodeId),
    Trigger(ast_node::NodeId),
    RTTrigger(ast_node::NodeId),
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum ComputeStep {
    Invoke(ast_node::NodeId),
    Extend(ast_node::NodeId),
    Evaluate(ast_node::NodeId),
    Terminate(ast_node::NodeId),
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum TimeOffset {
    UpToNow(Duration),
    Future(Duration),
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum Offset {
    Discrete(i32),
    Time(TimeOffset),
    SlidingWindow,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum Location {
    Invoke,
    Extend,
    Terminate,
    Expression,
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum StreamDependency {
    Access(Location, Offset, Span),
    InvokeByName(Span),
}

/// For every stream we need to store
/// - the last k+1 values if there is an access with discrete offset -k
/// - everything if its auxiliary streams depend on the future
///
/// We also have do differentiate between
/// - streams that do not (transitively) depend on the future and can therefore directly be computed
/// - streams that (transitively) depend on the future and therefore positions may not already be evaluated while in the buffer
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum StorageRequirement {
    Finite(u16),
    FutureRef(u16),
    Unbounded,
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum TrackingRequirement {
    Finite(u16),
    // TODO What about accessing a future dependent stream?
    Future,
    Unbounded,
}

pub(crate) type DependencyGraph = Graph<StreamNode, StreamDependency, Directed>;
pub(crate) type ComputationGraph = Graph<ComputeStep, (), Directed>;
//pub(crate) type NId = <DependencyGraph as petgraph::visit::GraphBase>::NodeId;
pub(crate) type NIx = petgraph::prelude::NodeIndex;
type EIx = petgraph::prelude::EdgeIndex;
//type EId = petgraph::prelude::EdgeIndex;

fn get_ast_id(dependent_node: StreamNode) -> NodeId {
    match dependent_node {
        StreamNode::ClassicOutput(id)
        | StreamNode::ParameterizedOutput(id)
        | StreamNode::RTOutput(id)
        | StreamNode::ClassicInput(id)
        | StreamNode::ParameterizedInput(id)
        | StreamNode::Trigger(id)
        | StreamNode::RTTrigger(id) => id,
    }
}
