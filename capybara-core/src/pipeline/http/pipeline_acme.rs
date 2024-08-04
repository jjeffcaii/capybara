use async_trait::async_trait;

use crate::pipeline::{HttpPipeline, HttpPipelineFactory, PipelineConf};

pub(crate) struct AcmePipeline {}

#[async_trait]
impl HttpPipeline for AcmePipeline {}

struct AcmePipelineFactory {}

impl HttpPipelineFactory for AcmePipelineFactory {
    type Item = AcmePipeline;

    fn generate(&self) -> anyhow::Result<Self::Item> {
        todo!()
    }
}

impl TryFrom<&PipelineConf> for AcmePipelineFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_acme_pipeline() -> anyhow::Result<()> {
        init();

        let c = {
            let s = r#"

            "#;

            serde_yaml::from_str::<'_, PipelineConf>(s)?
        };

        let f = AcmePipelineFactory::try_from(&c)?;
        let p = f.generate()?;

        Ok(())
    }
}
