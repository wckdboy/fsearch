//! Rooted index: the iOS shape (picked folders, explicit refresh), exercised
//! here on whatever OS the tests run on. macOS CI runs the same path the
//! simulator will.

use fsearch::{Engine, Options};
use std::path::PathBuf;
use std::time::Duration;

fn scratch(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "fsearch-ios-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn wait_ready(engine: &Engine) {
    for _ in 0..200 {
        if engine.progress().ready && engine.progress().phase == "ready" {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("index did not become ready: {:?}", engine.progress().phase);
}

#[test]
fn refresh_picks_up_adds_and_deletes() {
    let root = scratch("root");
    let index = scratch("index");
    std::fs::write(root.join("alpha.txt"), b"unique-token-ios-port\n").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("beta.rs"), b"fn beta() {}\n").unwrap();

    let engine = Engine::start_roots(Options { dir: index.clone(), home: "/fsearch-not-a-home".into(), skip: None }, vec![root.clone()]).unwrap();
    wait_ready(&engine);

    let hits = engine.search(&fsearch::Query::parse("alpha", "/fsearch-not-a-home").unwrap()).unwrap();
    assert!(hits.iter().any(|h| h.path.ends_with("alpha.txt")), "missing alpha ({} hits)", hits.len());
    let hits = engine.search(&fsearch::Query::parse("beta", "/fsearch-not-a-home").unwrap()).unwrap();
    assert!(hits.iter().any(|h| h.path.ends_with("beta.rs")), "missing beta");

    let mut saw = false;
    for _ in 0..50 {
        let q = fsearch::Query::parse("grep:unique-token-ios-port", "/fsearch-not-a-home").unwrap();
        let g = fsearch::Grep::new("unique-token-ios-port", fsearch::GrepMode::Literal).unwrap();
        if let Ok((r, _)) = engine.grep(&q, &g)
            && r.files.iter().any(|f| String::from_utf8_lossy(&f.path).contains("alpha.txt"))
        {
            saw = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(saw, "grep did not find the token under a root outside $HOME");

    std::fs::write(root.join("gamma.txt"), b"gamma\n").unwrap();
    let _ = std::fs::remove_file(root.join("alpha.txt"));
    engine.refresh().unwrap();

    let hits = engine.search(&fsearch::Query::parse("gamma", "/fsearch-not-a-home").unwrap()).unwrap();
    assert!(hits.iter().any(|h| h.path.ends_with("gamma.txt")), "refresh missed gamma");
    let hits = engine.search(&fsearch::Query::parse("alpha", "/fsearch-not-a-home").unwrap()).unwrap();
    assert!(!hits.iter().any(|h| h.path.ends_with("alpha.txt")), "refresh kept a deleted file");

    assert!(engine.status().owner);
    engine.stop();
    let again = Engine::start_roots(Options { dir: index.clone(), home: "/fsearch-not-a-home".into(), skip: None }, vec![root.clone()]).unwrap();
    wait_ready(&again);
    assert!(again.status().owner, "stop() should release the index lock");
    let hits = again.search(&fsearch::Query::parse("gamma", "/fsearch-not-a-home").unwrap()).unwrap();
    assert!(hits.iter().any(|h| h.path.ends_with("gamma.txt")));
    again.stop();

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&index);
}
