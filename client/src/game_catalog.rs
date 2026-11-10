use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub executables: Vec<String>,
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u64,
    pub games: Vec<Game>,
}
impl Catalog {
    pub fn valid(&self) -> bool {
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        serde_json::to_vec(&self.games).is_ok_and(|b| b.len() <= 24 * 1024)
            && self.version > 0
            && self.games.len() <= 64
            && self.games.iter().all(|g| {
                !g.id.is_empty()
                    && g.id.as_bytes()[0].is_ascii_alphanumeric()
                    && g.id.len() <= 48
                    && g.id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    && ids.insert(&g.id)
                    && !g.title.trim().is_empty()
                    && g.title.len() <= 96
                    && !g.title.contains(['\n', '\r', '\0'])
                    && !g.executables.is_empty()
                    && g.executables.len() <= 8
                    && g.executables.iter().all(|exe| {
                        let n = exe.to_ascii_lowercase();
                        n.len() > 4
                            && n.len() <= 96
                            && n.ends_with(".exe")
                            && n.trim() == n
                            && !n.contains([
                                '/', '\\', ':', '*', '?', '"', '<', '>', '|', '\n', '\r', '\0',
                            ])
                            && !matches!(
                                n.as_str(),
                                "java.exe"
                                    | "javaw.exe"
                                    | "steam.exe"
                                    | "epicgameslauncher.exe"
                                    | "riotclientservices.exe"
                                    | "cmd.exe"
                                    | "powershell.exe"
                                    | "explorer.exe"
                                    | "python.exe"
                                    | "pythonw.exe"
                                    | "node.exe"
                                    | "rundll32.exe"
                                    | "msedge.exe"
                                    | "chrome.exe"
                                    | "firefox.exe"
                                    | "battle.net.exe"
                            )
                            && names.insert(n)
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Catalog {
        Catalog {
            version: 1,
            games: vec![Game {
                id: "cs2".into(),
                title: "Counter-Strike 2".into(),
                executables: vec!["cs2.exe".into()],
                enabled: true,
            }],
        }
    }
    #[test]
    fn validates_untrusted_rules_atomically() {
        let c = catalog();
        assert!(c.valid());
        let mut c = catalog();
        c.games[0].executables = vec!["javaw.exe".into()];
        assert!(!c.valid());
        c.games[0].executables = vec!["../cs2.exe".into()];
        assert!(!c.valid());
        let mut c = catalog();
        c.games.push(c.games[0].clone());
        assert!(!c.valid());
        let mut c = catalog();
        c.version = 0;
        assert!(!c.valid());
    }
}
