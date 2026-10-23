use mediaa_core::agent::ModelSource;

pub fn local_spec(source: &ModelSource) -> anyhow::Result<String> {
    mediaa_engine::runtime::source_name(source)
}
