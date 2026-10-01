#[derive(Default, Debug)]
pub struct Cfg {
    pub force_image: bool,
    pub as_b64: bool,
    pub as_data: bool,
    pub from_llm: bool,
    pub trace: bool,
    pub help: bool,
    pub fmt: Option<String>,
    pub fence: String,
    pub positional: Option<String>,
    pub diff: bool,
    pub diff_target: Option<String>,
    pub view: bool,
    pub merge: bool,
    pub merge_ours: bool,
    pub merge_theirs: bool,
    pub merge_both: bool,
}

pub fn parse() -> Cfg {
    let mut c = Cfg {
        fence: "```".into(),
        ..Default::default()
    };

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0usize;

    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "/diff" | "--diff" => {
                c.diff = true;
                if let Some(target) = argv.get(i + 1) {
                    c.diff_target = Some(target.clone());
                    i += 1;
                }
            }
            "/image" | "/i" | "--i" | "-i" | "--image" => c.force_image = true,
            "/b64" | "/b" | "--b64" | "-b" | "--b" => c.as_b64 = true,
            "/data" | "/d" | "--data" | "-d" | "--d" => c.as_data = true,
            "/files" | "/file" | "/f" | "--files" | "--file" | "-f" | "--f" => {}
            "/llm" | "/l" | "--llm" | "--l" | "-llm" | "-l" => c.from_llm = true,
            "/trace" | "/t" | "--t" | "--trace" | "-t" => c.trace = true,
            "/help" | "/h" | "/?" | "--h" | "--help" | "-h" | "-?" | "?" => c.help = true,
            "/view" | "/v" | "-view" | "--view" | "-v" | "--v" => c.view  = true,
            "/merge" | "--merge" => c.merge = true,
            "/merge:ours" | "--merge:ours" => { c.merge = true; c.merge_ours = true; }
            "/merge:theirs" | "--merge:theirs" => { c.merge = true; c.merge_theirs = true; }
            "/merge:both" | "--merge:both" => { c.merge = true; c.merge_both = true; }
            s if s.starts_with("--fmt:") => c.fmt = Some(s[6..].to_lowercase()),
            s if s.starts_with("--fence:") => c.fence = s[8..].to_string(),
            s if s.starts_with("--") => {}
            s => {
                if c.positional.is_none() {
                    c.positional = Some(s.to_string());
                }
            }
        }
        i += 1;
    }
    c
}
