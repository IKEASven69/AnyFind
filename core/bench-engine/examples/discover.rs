fn main() {
    for ad in anyfind_core::adapters::ADAPTERS {
        let refs = (ad.discover)();
        let total: u64 = refs.iter().map(|r| r.size).sum();
        println!("{:12} sessions={} bytes={}", ad.name, refs.len(), total);
    }
    let (found, note) = anyfind_core::adapters::cursor_probe();
    println!("cursor probe: {:?} {}", found, note);
}
