//! Shared checkout identity for the desktop and organization daemon.
use std::path::PathBuf;
use std::process::Command;

pub struct Repo {
    pub checkout: String,
    pub key: String,
    pub main: Option<String>,
}

fn git(dir: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn path(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).trim_end_matches(['\r', '\n']).to_string();
    std::fs::canonicalize(&text).unwrap_or_else(|_| PathBuf::from(&text)).to_string_lossy().into_owned()
}

pub fn resolve(dir: &str) -> Option<Repo> {
    let checkout = path(&git(dir, &["rev-parse", "--show-toplevel"])?);
    let key = path(&git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
    // Git lists the main checkout first, even when invoked in a linked
    // checkout. NUL records preserve spaces, colons and newlines in paths.
    let worktrees = git(dir, &["worktree", "list", "--porcelain", "-z"])?;
    let record: Vec<_> = worktrees.split(|b| *b == 0).take_while(|field| !field.is_empty()).collect();
    let main = if record.iter().any(|field| *field == b"bare") {
        None
    } else {
        record.first().and_then(|field| field.strip_prefix(b"worktree ")).map(|p| {
            std::fs::canonicalize(PathBuf::from(String::from_utf8_lossy(p).as_ref()))
                .unwrap_or_else(|_| PathBuf::from(String::from_utf8_lossy(p).as_ref()))
                .to_string_lossy().into_owned()
        })
    };
    Some(Repo { checkout, key, main })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub struct Fixture { pub root: PathBuf, pub main: PathBuf, pub linked: PathBuf }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }
    pub fn fixture() -> Fixture {
        let root = std::env::temp_dir().join(format!("lazed-repo-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let main = root.join("main repo");
        let linked = root.join("linked checkout");
        std::fs::create_dir_all(&main).unwrap();
        for args in [vec!["init", "-q"], vec!["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "fixture"], vec!["worktree", "add", "-qb", "fixture", linked.to_str().unwrap()]] {
            assert!(Command::new("git").arg("-C").arg(&main).args(args).output().unwrap().status.success());
        }
        Fixture { root, main: std::fs::canonicalize(main).unwrap(), linked: std::fs::canonicalize(linked).unwrap() }
    }
    #[test]
    fn linked_checkout_and_subdirectory_resolve_to_the_actual_main() {
        let fixture = fixture();
        std::fs::create_dir_all(fixture.linked.join("src")).unwrap();
        let repo = resolve(fixture.linked.join("src").to_str().unwrap()).unwrap();
        assert_eq!(repo.main.as_deref(), fixture.main.to_str());
        assert_eq!(repo.checkout, fixture.linked.to_str().unwrap());
        assert_eq!(repo.key, resolve(fixture.main.to_str().unwrap()).unwrap().key);
    }
}
