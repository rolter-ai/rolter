//! LDAP directories and their group→role mappings (#1826).

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{LdapDirectory, LdapGroupMapping};
use super::sso::SecretUpdate;
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// The LDAP directory an org signs its members in through, and the mappings
/// that turn the directory's groups into rolter grants.
pub struct LdapRepo<'a>(pub &'a PgPool);

const LDAP_DIRECTORY_COLUMNS: &str = "id, org_id, name, url, bind_dn, bind_secret_ciphertext, \
     bind_secret_nonce, base_dn, user_filter, email_attr, display_name_attr, group_attr, \
     require_tls, default_role, enabled, created_at";

const LDAP_MAPPING_COLUMNS: &str =
    "id, directory_id, group_name, org_id, team_id, project_id, role, created_at";

/// The settings of an LDAP directory, as written on create and on update.
///
/// A struct rather than an argument list: eleven columns, most of them strings,
/// is a silent swap waiting to happen.
#[derive(Debug, Clone, Copy)]
pub struct LdapDirectoryValues<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub bind_dn: &'a str,
    pub base_dn: &'a str,
    pub user_filter: &'a str,
    pub email_attr: &'a str,
    pub display_name_attr: &'a str,
    pub group_attr: &'a str,
    pub require_tls: bool,
    pub default_role: Option<&'a str>,
    pub enabled: bool,
}

impl LdapRepo<'_> {
    /// Register the org's directory. An org has at most one, so a second create
    /// surfaces as [`rolter_core::Error::AlreadyExists`].
    pub async fn create_directory(
        &self,
        org_id: Uuid,
        values: LdapDirectoryValues<'_>,
        bind_secret: Option<(&[u8], &[u8])>,
    ) -> Result<LdapDirectory> {
        let (ciphertext, nonce) = match bind_secret {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into ldap_directories (org_id, name, url, bind_dn, bind_secret_ciphertext, \
                    bind_secret_nonce, base_dn, user_filter, email_attr, display_name_attr, \
                    group_attr, require_tls, default_role, enabled) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
             returning {LDAP_DIRECTORY_COLUMNS}"
        )))
        .bind(org_id)
        .bind(values.name)
        .bind(values.url)
        .bind(values.bind_dn)
        .bind(ciphertext)
        .bind(nonce)
        .bind(values.base_dn)
        .bind(values.user_filter)
        .bind(values.email_attr)
        .bind(values.display_name_attr)
        .bind(values.group_attr)
        .bind(values.require_tls)
        .bind(values.default_role)
        .bind(values.enabled)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Replace the directory's settings. The sealed bind password is
    /// three-valued - kept, cleared or replaced - which the `case when $3` pair
    /// expresses in one statement, so a rotation can never leave the row with a
    /// ciphertext from one write and a nonce from another.
    pub async fn update_directory(
        &self,
        id: Uuid,
        values: LdapDirectoryValues<'_>,
        bind_secret: SecretUpdate<'_>,
    ) -> Result<LdapDirectory> {
        let (replace, ciphertext, nonce) = match bind_secret {
            SecretUpdate::Keep => (false, None, None),
            SecretUpdate::Clear => (true, None, None),
            SecretUpdate::Set(c, n) => (true, Some(c), Some(n)),
        };
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update ldap_directories set \
                    name = $2, url = $3, bind_dn = $4, \
                    bind_secret_ciphertext = case when $5 then $6 else bind_secret_ciphertext end, \
                    bind_secret_nonce = case when $5 then $7 else bind_secret_nonce end, \
                    base_dn = $8, user_filter = $9, email_attr = $10, display_name_attr = $11, \
                    group_attr = $12, require_tls = $13, default_role = $14, enabled = $15 \
                 where id = $1 returning {LDAP_DIRECTORY_COLUMNS}"
            )))
            .bind(id)
            .bind(values.name)
            .bind(values.url)
            .bind(values.bind_dn)
            .bind(replace)
            .bind(ciphertext)
            .bind(nonce)
            .bind(values.base_dn)
            .bind(values.user_filter)
            .bind(values.email_attr)
            .bind(values.display_name_attr)
            .bind(values.group_attr)
            .bind(values.require_tls)
            .bind(values.default_role)
            .bind(values.enabled),
            self.0,
            || format!("ldap directory {id}"),
        )
        .await
    }

    pub async fn get_directory(&self, id: Uuid) -> Result<LdapDirectory> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {LDAP_DIRECTORY_COLUMNS} from ldap_directories where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("ldap directory {id}"),
        )
        .await
    }

    /// The org's directory, if it registered one.
    pub async fn find_for_org(&self, org_id: Uuid) -> Result<Option<LdapDirectory>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {LDAP_DIRECTORY_COLUMNS} from ldap_directories where org_id = $1"
        )))
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Every directory a password sign-in may be tried against: enabled ones,
    /// oldest first so the order a login walks them in is stable.
    pub async fn list_enabled(&self) -> Result<Vec<LdapDirectory>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {LDAP_DIRECTORY_COLUMNS} from ldap_directories \
             where enabled order by created_at, id"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether any directory is enabled, for the login screen's discovery
    /// endpoint. A cheap existence probe, not a row read.
    pub async fn any_enabled(&self) -> Result<bool> {
        sqlx::query_scalar("select exists (select 1 from ldap_directories where enabled)")
            .fetch_one(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn delete_directory(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from ldap_directories where id = $1").bind(id),
            self.0,
            || format!("ldap directory {id}"),
        )
        .await?;
        Ok(())
    }

    /// Open the sealed service-account password. `None` is a directory that
    /// allows an anonymous search.
    pub async fn bind_password(
        &self,
        kek: &super::super::crypto::Kek,
        directory: &LdapDirectory,
    ) -> Result<Option<String>> {
        match (
            &directory.bind_secret_ciphertext,
            &directory.bind_secret_nonce,
        ) {
            (Some(c), Some(n)) => Ok(Some(kek.decrypt(c, n)?)),
            _ => Ok(None),
        }
    }

    pub async fn add_mapping(
        &self,
        directory_id: Uuid,
        group_name: &str,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
    ) -> Result<LdapGroupMapping> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into ldap_group_mappings (directory_id, group_name, org_id, team_id, \
                    project_id, role) \
             values ($1, $2, $3, $4, $5, $6) returning {LDAP_MAPPING_COLUMNS}"
        )))
        .bind(directory_id)
        .bind(group_name)
        .bind(org_id)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list_mappings(&self, directory_id: Uuid) -> Result<Vec<LdapGroupMapping>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {LDAP_MAPPING_COLUMNS} from ldap_group_mappings \
             where directory_id = $1 order by group_name, role"
        )))
        .bind(directory_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_mapping(&self, id: Uuid) -> Result<LdapGroupMapping> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {LDAP_MAPPING_COLUMNS} from ldap_group_mappings where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("ldap group mapping {id}"),
        )
        .await
    }

    pub async fn delete_mapping(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from ldap_group_mappings where id = $1").bind(id),
            self.0,
            || format!("ldap group mapping {id}"),
        )
        .await?;
        Ok(())
    }
}
