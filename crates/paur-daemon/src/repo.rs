//! Build a [`paur_repo::RepoCtx`] from a [`paur_core::Config`].

use paur_core::Config;
use paur_repo::RepoCtx;

/// Construct a signing context from the daemon config + the GPG key id
/// stored in DB settings. If no key id is set yet, this returns an
/// error — the caller (typically `paur init`) must run first.
pub async fn build_repo_ctx(
    cfg: &Config,
    db: &paur_db::Db,
) -> Result<RepoCtx, paur_core::Error> {
    let key = db
        .get_setting("gpg_key_id")
        .await?
        .ok_or_else(|| paur_core::Error::Gpg("gpg_key_id not set; run `paur init`".into()))?;
    Ok(RepoCtx {
        repo_name: cfg.repo_name.clone(),
        arch: cfg.arch.clone(),
        repo_dir: cfg.repo_dir.clone(),
        gpg_home: cfg.gpg_home.clone(),
        gpg_key: key,
        container_runtime: cfg.container_runtime,
        builder_image: cfg.builder_image.clone(),
        s3: cfg.s3.clone(),
    })
}
