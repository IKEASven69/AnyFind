fn main() {
    for root in [r"C:\Users\20369\.cursor\skills", r"C:\Users\20369\.config\opencode\skills", r"C:\Users\20369\.claude\skills"] {
        let mut ok = 0; let mut errs = 0; let mut long = 0;
        for e in walkdir::WalkDir::new(root) {
            match e {
                Ok(en) => {
                    let p = en.path();
                    if p.is_file() && p.extension().and_then(|s| s.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("md")) {
                        ok += 1;
                    }
                }
                Err(er) => { errs += 1; if er.path().map(|p| p.to_string_lossy().len() > 240).unwrap_or(false) { long += 1; } }
            }
        }
        println!("{root}: md={ok} walk_errors={errs} long_path_errs={long}");
    }
}
