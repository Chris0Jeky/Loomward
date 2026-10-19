//! Portable coverage of the production hint resolver and targeted-start decision (#794).
//! These are catalogue fixtures, with no native grant, watcher, or filesystem traversal.
use super::{hint_covered, resolve_hint};
use crate::{Config, Service};
use loomward_protocol::DatasetClass;
use rusqlite::params;

fn path(value: &str) -> Vec<Vec<u16>> {
    value
        .split('/')
        .map(|name| name.encode_utf16().collect())
        .collect()
}

#[test]
fn targeted_start_does_not_cover_a_renamed_known_descendant_through_its_ancestor() {
    for (case, parent_name, new_name) in [
        ("rename", "c", "renamed"),
        ("move", "b", "sub1"),
        ("case rename", "c", "SUB1"),
    ] {
        let temp = crate::tests::temp_root();
        let svc = Service::open(Config {
            state_dir: temp.path().to_path_buf(),
            dataset: DatasetClass::Synthetic,
            allow_personal: false,
            allow_identity_unverified: false,
            grant_roots: vec![],
        })
        .unwrap();
        svc.inner.freshness.stop();
        let (root, _) = crate::tests::fixture(&svc, 0);
        let root_dir: i64 = svc
            .inner
            .db()
            .query_row(
                "SELECT id FROM main.dir WHERE root_id=?1 AND parent_id IS NULL",
                [root],
                |r| r.get(0),
            )
            .unwrap();
        let insert = |parent: i64, name: &str, depth: i64, id: u8| {
            let conn = svc.inner.db();
            conn.execute(
                "INSERT INTO main.dir(root_id,parent_id,name,file_id,id_quality,id_basis,depth,attrs,listing_state,born_run,seen_run)
                 VALUES(?1,?2,?3,?4,'native_file_id_128','listed',?5,16,'complete',0,0)",
                params![root, parent, name, vec![id; 16], depth],
            ).unwrap();
            conn.last_insert_rowid()
        };
        let c = insert(root_dir, "c", 1, 10);
        let b = insert(root_dir, "b", 1, 11);
        let child = insert(c, "sub1", 2, 12);
        let inner = insert(child, "inner", 3, 13) as u64;
        let old_path = path("c/sub1/inner");
        assert_eq!(
            resolve_hint(&svc.inner, root, &old_path),
            Some((inner, true))
        );
        assert!(hint_covered(&svc.inner, root, &old_path, &[inner]));
        assert!(!hint_covered(&svc.inner, root, &old_path, &[c as u64]));

        // A write below the relocated directory names a path the catalogue has not seen yet.
        // The bounded scan can list its ancestor and retain this identity's complete listing.
        let parent = if parent_name == "c" { c } else { b };
        let hint = path(&format!("{parent_name}/{new_name}/inner"));
        assert_eq!(
            resolve_hint(&svc.inner, root, &hint),
            Some((parent as u64, false))
        );
        assert!(
            !hint_covered(&svc.inner, root, &hint, &[parent as u64]),
            "{case}: an ancestor-only start must retain full recovery"
        );

        // Model the parent's identity-preserving publication, without relisting its descendant.
        svc.inner
            .db()
            .execute(
                "UPDATE main.dir SET parent_id=?1,name=?2 WHERE id=?3",
                params![parent, new_name, child],
            )
            .unwrap();
        let listing: String = svc
            .inner
            .db()
            .query_row(
                "SELECT listing_state FROM main.dir WHERE id=?1",
                [child],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(listing, "complete", "the known child may be skipped");
        assert_eq!(resolve_hint(&svc.inner, root, &hint), Some((inner, true)));
        assert!(!hint_covered(&svc.inner, root, &hint, &[parent as u64]));
        assert!(hint_covered(&svc.inner, root, &hint, &[inner]));

        for state in ["absent_pending", "excluded"] {
            svc.inner
                .db()
                .execute(
                    "UPDATE main.dir SET listing_state=?1 WHERE id=?2",
                    params![state, child],
                )
                .unwrap();
            assert_eq!(resolve_hint(&svc.inner, root, &hint), None);
            assert!(!hint_covered(&svc.inner, root, &hint, &[inner]));
        }
        assert!(!hint_covered(&svc.inner, root, &[vec![0xd800]], &[inner]));
        assert!(!hint_covered(&svc.inner, i64::MAX, &[], &[root_dir as u64]));
    }
}
