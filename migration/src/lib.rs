pub use sea_orm_migration::prelude::*;

mod m20260910_000001_create_initial_tables;
mod m20260910_000002_add_is_sandbox_to_saves;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260910_000001_create_initial_tables::Migration),
            Box::new(m20260910_000002_add_is_sandbox_to_saves::Migration),
        ]
    }
}
