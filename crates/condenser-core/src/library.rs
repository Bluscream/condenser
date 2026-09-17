use crate::config::AppPaths;
use crate::error::{Error, Result};
use crate::model::Game;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// The persisted game library. Written atomically so a crash mid-save can't
/// leave the user with an empty library.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub games: Vec<Game>,
}

impl Library {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        let file = paths.library_file();
        if !file.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&file)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        paths.ensure()?;
        let file = paths.library_file();
        let tmp = file.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, &file)?;
        Ok(())
    }

    pub fn get(&self, id: Uuid) -> Result<&Game> {
        self.games
            .iter()
            .find(|g| g.id == id)
            .ok_or(Error::GameNotFound(id))
    }

    pub fn get_mut(&mut self, id: Uuid) -> Result<&mut Game> {
        self.games
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or(Error::GameNotFound(id))
    }

    pub fn remove(&mut self, id: Uuid) -> Result<Game> {
        let idx = self
            .games
            .iter()
            .position(|g| g.id == id)
            .ok_or(Error::GameNotFound(id))?;
        Ok(self.games.remove(idx))
    }

    /// Add a game, allocating it a dedicated Proton prefix under the data dir.
    ///
    /// Returns the new game's id rather than a reference: the caller usually wants the
    /// id anyway, and it avoids re-borrowing the vector just to hand back the element
    /// we already know the identity of.
    pub fn add(&mut self, title: impl Into<String>, executable: PathBuf, paths: &AppPaths) -> Uuid {
        let id = Uuid::new_v4();
        let prefix = paths.prefixes_dir().join(id.to_string());
        let mut game = Game::new(title.into(), executable, prefix);
        game.id = id;
        self.games.push(game);
        id
    }

    /// Library sorted for display: most recently played first, then alphabetical.
    #[must_use]
    pub fn sorted(&self) -> Vec<&Game> {
        let mut out: Vec<&Game> = self.games.iter().collect();
        out.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(dir: &std::path::Path) -> AppPaths {
        AppPaths {
            data: dir.join("data"),
            config: dir.join("config"),
        }
    }

    #[test]
    fn roundtrip_persists_games() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        let mut lib = Library::default();
        lib.add("Portal 2", PathBuf::from("/games/p2/portal2.exe"), &paths);
        lib.save(&paths).unwrap();

        let loaded = Library::load(&paths).unwrap();
        assert_eq!(loaded.games.len(), 1);
        assert_eq!(loaded.games[0].title, "Portal 2");
        // each game gets its own prefix
        assert!(loaded.games[0]
            .prefix
            .starts_with(paths.prefixes_dir()));
    }

    #[test]
    fn sorted_puts_recent_first() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let mut lib = Library::default();
        lib.add("Zeta", PathBuf::from("/a.exe"), &paths);
        lib.add("Alpha", PathBuf::from("/b.exe"), &paths);
        let zeta = lib.games[0].id;
        lib.get_mut(zeta).unwrap().last_played = Some(1000);

        let sorted = lib.sorted();
        assert_eq!(sorted[0].title, "Zeta");
        assert_eq!(sorted[1].title, "Alpha");
    }
}
