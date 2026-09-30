use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("missing state parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut nonce)).map_err(|e| e.to_string())?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = parent.join(format!(".lazed-{nonce}.tmp"));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() { let _ = std::fs::remove_file(tmp); }
    result.map_err(|e| format!("state persistence failed: {e}"))
}
