pub mod v1;
pub mod v2;

use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    transforms::Transform,
};
use serde::{Deserialize, Serialize};
use vector_core::event::Event;

#[derive(Serialize, Deserialize, Debug, Clone)]
enum V1 {
    #[serde(rename = "1")]
    V1,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LuaConfigV1 {
    version: Option<V1>,
    #[serde(flatten)]
    config: v1::LuaConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
enum V2 {
    #[serde(rename = "2")]
    V2,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LuaConfigV2 {
    version: V2,
    #[serde(flatten)]
    config: v2::LuaConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum LuaConfig {
    V1(LuaConfigV1),
    V2(LuaConfigV2),
}

inventory::submit! {
    TransformDescription::new::<LuaConfig>("lua")
}

impl GenerateConfig for LuaConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(
            r#"version = "2"
            hooks.process = """#,
        )
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "lua")]
impl TransformConfig for LuaConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        match self {
            LuaConfig::V1(v1) => v1.config.build(),
            LuaConfig::V2(v2) => v2.config.build(),
        }
    }

    fn input_type(&self) -> DataType {
        match self {
            LuaConfig::V1(v1) => v1.config.input_type(),
            LuaConfig::V2(v2) => v2.config.input_type(),
        }
    }

    fn output_type(&self) -> DataType {
        match self {
            LuaConfig::V1(v1) => v1.config.output_type(),
            LuaConfig::V2(v2) => v2.config.output_type(),
        }
    }

    fn transform_type(&self) -> &'static str {
        match self {
            LuaConfig::V1(v1) => v1.config.transform_type(),
            LuaConfig::V2(v2) => v2.config.transform_type(),
        }
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<super::LuaConfig>();
    }
}
