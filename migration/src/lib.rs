pub use sea_orm_migration::prelude::*;

mod m20260910_000001_create_initial_tables;
mod m20260910_000002_add_is_sandbox_to_saves;
mod m20260910_000003_add_request_id_to_commands;
mod m20260910_000004_create_pair_messages;
mod m20260910_000005_create_pair_threads;
mod m20260910_000006_add_refs_to_pair_messages;
mod m20260910_000007_add_pending_suggestions_to_pair_threads;
mod m20260912_000008_add_reasoning_to_pair_messages;
mod m20260913_000009_add_model_to_saves;
mod m20260913_000010_add_reasoning_effort_to_saves;
mod m20260913_000011_add_attachments_to_pair_messages;
mod m20260914_000012_add_narrative_to_saves;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260910_000001_create_initial_tables::Migration),
            Box::new(m20260910_000002_add_is_sandbox_to_saves::Migration),
            Box::new(m20260910_000003_add_request_id_to_commands::Migration),
            Box::new(m20260910_000004_create_pair_messages::Migration),
            Box::new(m20260910_000005_create_pair_threads::Migration),
            Box::new(m20260910_000006_add_refs_to_pair_messages::Migration),
            Box::new(m20260910_000007_add_pending_suggestions_to_pair_threads::Migration),
            Box::new(m20260912_000008_add_reasoning_to_pair_messages::Migration),
            Box::new(m20260913_000009_add_model_to_saves::Migration),
            Box::new(m20260913_000010_add_reasoning_effort_to_saves::Migration),
            Box::new(m20260913_000011_add_attachments_to_pair_messages::Migration),
            Box::new(m20260914_000012_add_narrative_to_saves::Migration),
        ]
    }
}
