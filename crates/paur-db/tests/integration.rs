//! Integration tests for paur-db. Uses an in-memory SQLite database to
//! exercise the full schema and CRUD surface.

use std::path::Path;

use paur_db::{BuildStatus, BuildTrigger, Stream};

#[tokio::test]
async fn package_upsert_and_read() {
    let db = paur_db::open(Path::new(":memory:")).await.unwrap();
    let id1 = db
        .upsert_package("paru-bin", "https://aur.archlinux.org/paru-bin.git", false)
        .await
        .unwrap();
    assert!(id1 > 0);
    let p = db.get_package_by_name("paru-bin").await.unwrap().unwrap();
    assert_eq!(p.name, "paru-bin");
    assert_eq!(p.aur_url, "https://aur.archlinux.org/paru-bin.git");
    assert!(!p.auto_rebuild);

    // Upsert again — same id, auto_rebuild is sticky (max with existing).
    let id2 = db
        .upsert_package("paru-bin", "https://aur.archlinux.org/paru-bin.git", true)
        .await
        .unwrap();
    assert_eq!(id1, id2);
    let p2 = db.get_package_by_name("paru-bin").await.unwrap().unwrap();
    assert!(p2.auto_rebuild);
}

#[tokio::test]
async fn build_enqueue_claim_finish() {
    let db = paur_db::open(Path::new(":memory:")).await.unwrap();
    let pkg_id = db
        .upsert_package("yay", "https://aur.archlinux.org/yay.git", false)
        .await
        .unwrap();

    let b1 = db
        .enqueue_build(pkg_id, BuildTrigger::Manual)
        .await
        .unwrap();
    let b2 = db
        .enqueue_build(pkg_id, BuildTrigger::Poll)
        .await
        .unwrap();
    assert!(b1 < b2);

    // Claim the older one.
    let claimed = db
        .claim_next_queued("worker-1")
        .await
        .unwrap()
        .expect("first claim must succeed");
    assert_eq!(claimed.id, b1);
    assert_eq!(claimed.status, BuildStatus::Running);
    assert_eq!(claimed.trigger, BuildTrigger::Manual);
    assert_eq!(claimed.worker_id.as_deref(), Some("worker-1"));

    // No more running slots in this test (default max_workers=1 not enforced
    // at db layer; daemon enforces) — second claim grabs the second queued.
    let claimed2 = db
        .claim_next_queued("worker-2")
        .await
        .unwrap()
        .expect("second claim must succeed");
    assert_eq!(claimed2.id, b2);

    // Queue is empty now.
    assert!(db.claim_next_queued("worker-3").await.unwrap().is_none());

    // Finish the first.
    db.finish_build(b1, BuildStatus::Success, Some(0))
        .await
        .unwrap();
    let b1_after = db.get_build(b1).await.unwrap().unwrap();
    assert_eq!(b1_after.status, BuildStatus::Success);
    assert_eq!(b1_after.exit_code, Some(0));
    assert!(b1_after.finished_at.is_some());

    // Finish the second one too.
    db.finish_build(b2, BuildStatus::Success, Some(0))
        .await
        .unwrap();

    // The most recent queued_at row is b2; latest_build_for returns it.
    let latest = db.latest_build_for(pkg_id).await.unwrap().unwrap();
    assert_eq!(latest.id, b2);
    assert_eq!(latest.status, BuildStatus::Success);
}

#[tokio::test]
async fn reap_stale_running() {
    let db = paur_db::open(Path::new(":memory:")).await.unwrap();
    let pkg_id = db
        .upsert_package("spotify", "https://aur.archlinux.org/spotify.git", false)
        .await
        .unwrap();
    let b = db.enqueue_build(pkg_id, BuildTrigger::Manual).await.unwrap();
    let _claimed = db.claim_next_queued("lost-worker").await.unwrap().unwrap();

    // Simulate a daemon restart: reaps the running build.
    let n = db.reap_stale_running().await.unwrap();
    assert_eq!(n, 1);
    let b_after = db.get_build(b).await.unwrap().unwrap();
    assert_eq!(b_after.status, BuildStatus::Failed);
    assert_eq!(b_after.exit_code, Some(-1));
}

#[tokio::test]
async fn logs_append_and_read() {
    let db = paur_db::open(Path::new(":memory:")).await.unwrap();
    let pkg_id = db
        .upsert_package("foo", "https://aur.archlinux.org/foo.git", false)
        .await
        .unwrap();
    let b = db.enqueue_build(pkg_id, BuildTrigger::Manual).await.unwrap();

    db.append_log(b, Stream::Stdout, "==> Making package: foo").await.unwrap();
    db.append_log(b, Stream::Stdout, "==> Tidying install").await.unwrap();
    db.append_log(b, Stream::Stderr, "warning: dependency").await.unwrap();

    let logs = db.read_logs(b).await.unwrap();
    assert_eq!(logs.len(), 3);
    assert_eq!(logs[0].1, "==> Making package: foo");
    assert_eq!(logs[2].0, Stream::Stderr);
}

#[tokio::test]
async fn settings_crud() {
    let db = paur_db::open(Path::new(":memory:")).await.unwrap();
    assert!(db.get_setting("gpg_key_id").await.unwrap().is_none());
    db.set_setting("gpg_key_id", "ABCDEF1234567890").await.unwrap();
    assert_eq!(
        db.get_setting("gpg_key_id").await.unwrap().as_deref(),
        Some("ABCDEF1234567890")
    );
    db.set_setting("gpg_key_id", "REPLACED").await.unwrap();
    assert_eq!(
        db.get_setting("gpg_key_id").await.unwrap().as_deref(),
        Some("REPLACED")
    );
}

#[tokio::test]
async fn list_builds_filters() {
    let db = paur_db::open(Path::new(":memory:")).await.unwrap();
    let p1 = db
        .upsert_package("p1", "https://aur.archlinux.org/p1.git", false)
        .await
        .unwrap();
    let p2 = db
        .upsert_package("p2", "https://aur.archlinux.org/p2.git", false)
        .await
        .unwrap();
    db.enqueue_build(p1, BuildTrigger::Manual).await.unwrap();
    db.enqueue_build(p2, BuildTrigger::Manual).await.unwrap();
    db.enqueue_build(p1, BuildTrigger::Poll).await.unwrap();

    let all = db.list_builds(None, None, 100).await.unwrap();
    assert_eq!(all.len(), 3);

    let only_p1 = db.list_builds(Some("p1"), None, 100).await.unwrap();
    assert_eq!(only_p1.len(), 2);

    let queued = db
        .list_builds(None, Some(BuildStatus::Queued), 100)
        .await
        .unwrap();
    assert_eq!(queued.len(), 3);

    let with_limit = db.list_builds(None, None, 2).await.unwrap();
    assert_eq!(with_limit.len(), 2);
}
