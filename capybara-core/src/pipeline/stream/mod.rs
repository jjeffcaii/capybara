pub use pipeline::{StreamContext, StreamPipeline};
pub(crate) use registry::{load, StreamPipelineFactoryExt};
pub use registry::{register, StreamPipelineFactory};

pub(crate) use pipeline_router::{RouteStreamPipeline, RouteStreamPipelineFactory};

mod pipeline;
mod pipeline_router;
mod registry;
