use sea_orm_migration::prelude::*;

/// 多账户（系统侧）：users / sessions 两张表，并给 saves / storybooks 加上归属列。
///
/// 归属语义（见 docs/auth-accounts.md）：
/// - `saves.owner_id` 非空即「谁开的档谁看得见」，别人一律 404；
/// - `storybooks.owner_id` 为作者；已发布的书对所有人可读可开档，但只有作者能改。
///
/// 列先建成可空：历史数据由引擎侧 `ensure_default_admin` 回填给默认管理员
/// （迁移器不能依赖引擎，所以常量 `DEFAULT_ADMIN_USER_ID` 只在引擎里有一份）。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. users：账户。username 一律以小写形态存储，唯一约束即大小写不敏感。
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Users::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Users::Username).string().not_null().unique_key())
                    .col(ColumnDef::new(Users::DisplayName).string().not_null())
                    .col(ColumnDef::new(Users::PasswordHash).text().not_null())
                    .col(ColumnDef::new(Users::IsAdmin).boolean().not_null().default(false))
                    .col(
                        ColumnDef::new(Users::MustChangePassword)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Users::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Users::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(Users::LastLoginAt).string().null())
                    .to_owned(),
            )
            .await?;

        // 2. sessions：登录态。只存令牌的 sha256，泄库也拿不到可用令牌。
        manager
            .create_table(
                Table::create()
                    .table(Sessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Sessions::TokenHash)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Sessions::UserId).string().not_null())
                    .col(ColumnDef::new(Sessions::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Sessions::ExpiresAt).string().not_null())
                    .col(ColumnDef::new(Sessions::LastSeenAt).string().not_null())
                    .col(ColumnDef::new(Sessions::UserAgent).string().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sessions_user")
                    .table(Sessions::Table)
                    .col(Sessions::UserId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // 3. 归属列（可空：新建时写入，历史数据由引擎回填）
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .add_column(ColumnDef::new(Saves::OwnerId).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Storybooks::Table)
                    .add_column(ColumnDef::new(Storybooks::OwnerId).string().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_saves_owner")
                    .table(Saves::Table)
                    .col(Saves::OwnerId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_storybooks_owner")
                    .table(Storybooks::Table)
                    .col(Storybooks::OwnerId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite 不允许 drop 被索引引用的列：先摘索引再摘列。
        for idx in ["idx_storybooks_owner", "idx_saves_owner", "idx_sessions_user"] {
            manager
                .drop_index(Index::drop().name(idx).if_exists().to_owned())
                .await?;
        }
        manager
            .alter_table(
                Table::alter()
                    .table(Storybooks::Table)
                    .drop_column(Storybooks::OwnerId)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Saves::Table)
                    .drop_column(Saves::OwnerId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Sessions::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Users::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Username,
    DisplayName,
    PasswordHash,
    IsAdmin,
    MustChangePassword,
    CreatedAt,
    UpdatedAt,
    LastLoginAt,
}

#[derive(DeriveIden)]
enum Sessions {
    Table,
    TokenHash,
    UserId,
    CreatedAt,
    ExpiresAt,
    LastSeenAt,
    UserAgent,
}

#[derive(DeriveIden)]
enum Saves {
    Table,
    OwnerId,
}

#[derive(DeriveIden)]
enum Storybooks {
    Table,
    OwnerId,
}
