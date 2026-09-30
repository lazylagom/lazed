use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Gate(Mutex<HashSet<String>>);
pub struct Permit { gate: Arc<Gate>, id: String }
impl Gate {
    pub fn acquire(self: &Arc<Self>, id: &str) -> Result<Permit, String> {
        let mut active = self.0.lock().map_err(|e| e.to_string())?;
        if active.contains(id) { return Err("automation_busy: this automation is already running".into()); }
        if active.len() >= 2 { return Err("automation_busy: two automations are running; retry later".into()); }
        active.insert(id.to_string());
        Ok(Permit { gate: self.clone(), id: id.to_string() })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.0.lock() { active.remove(&self.id); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_slow_run_does_not_block_another_but_duplicates_and_a_third_are_refused() {
        let gate = Arc::new(Gate::default());
        let first = gate.acquire("slow").unwrap();
        let other = gate.clone();
        std::thread::spawn(move || {
            let second = other.acquire("fast").unwrap();
            assert!(other.acquire("slow").is_err());
            assert!(other.acquire("third").is_err());
            drop(second);
            assert!(other.acquire("third").is_ok());
        }).join().unwrap();
        drop(first);
        assert!(gate.acquire("slow").is_ok());
    }
}
