//! Folders a sync service keeps: a Library there is refused, chosen or
//! opened, since syncing files while the app writes them can damage it.
//!
//! Told by the path: iCloud Drive (`~/Library/Mobile Documents`, Desktop
//! and Documents when synced, `~/iCloudDrive` on Windows), File Providers in
//! `~/Library/CloudStorage`, Dropbox's `.dropbox` mark, OneDrive's folders
//! and Google Drive's (`~/Google Drive`, `My Drive` and `Shared drives` on
//! Windows).

use std::path::{Component, Path, PathBuf};

/// The service that syncs a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncService {
    ICloudDrive,
    Dropbox,
    OneDrive,
    GoogleDrive,
    /// Another File Provider's folder, by the name it gives it ("Box").
    Other(String),
}

impl SyncService {
    /// The service's own name, as it calls itself.
    pub fn name(&self) -> &str {
        match self {
            Self::ICloudDrive => "iCloud Drive",
            Self::Dropbox => "Dropbox",
            Self::OneDrive => "OneDrive",
            Self::GoogleDrive => "Google Drive",
            Self::Other(name) => name,
        }
    }
}

/// The service that syncs `folder` (made or not), as far as its path tells.
pub fn sync_service(folder: &Path) -> Option<SyncService> {
    let home = dirs::home_dir();
    let one_drive: Vec<PathBuf> = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
        .collect();
    // A link is followed where it exists.
    let folder = crate::frames::sources::canonical(folder).unwrap_or_else(|_| folder.to_path_buf());
    let around = Around { home: home.as_deref(), one_drive: &one_drive };
    service_of(&folder, &around, &|p| p.exists())
}

/// Where the services keep their folders on this computer.
struct Around<'a> {
    home: Option<&'a Path>,
    /// OneDrive's folders, as its environment variables name them.
    one_drive: &'a [PathBuf],
}

fn service_of(
    folder: &Path,
    around: &Around,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<SyncService> {
    if let Some(home) = around.home {
        let icloud = home.join("Library/Mobile Documents");
        if folder.starts_with(&icloud) || folder.starts_with(home.join("iCloudDrive")) {
            return Some(SyncService::ICloudDrive);
        }
        // Desktop & Documents Folders: iCloud Drive holds both.
        let drive = icloud.join("com~apple~CloudDocs");
        for name in ["Desktop", "Documents"] {
            if folder.starts_with(home.join(name)) && exists(&drive.join(name)) {
                return Some(SyncService::ICloudDrive);
            }
        }
        if let Ok(rest) = folder.strip_prefix(home.join("Library/CloudStorage")) {
            let provider = first_name(rest)?;
            return Some(provider_service(&provider));
        }
        if let Ok(rest) = folder.strip_prefix(home) {
            match first_name(rest).as_deref() {
                Some(name) if name.starts_with("OneDrive") => return Some(SyncService::OneDrive),
                Some("Google Drive") => return Some(SyncService::GoogleDrive),
                _ => {}
            }
        }
    }
    if around.one_drive.iter().any(|d| !d.as_os_str().is_empty() && folder.starts_with(d)) {
        return Some(SyncService::OneDrive);
    }
    // Google Drive's drive on Windows: `G:\My Drive`, `G:\Shared drives`.
    let mut normal = folder.components().skip_while(|c| !matches!(c, Component::Normal(_)));
    if let Some(Component::Normal(top)) = normal.next()
        && folder.has_root()
        && matches!(top.to_str(), Some("My Drive" | "Shared drives"))
        && cfg!(windows)
    {
        return Some(SyncService::GoogleDrive);
    }
    if folder.ancestors().any(|a| exists(&a.join(".dropbox"))) {
        return Some(SyncService::Dropbox);
    }
    None
}

/// The first folder's name in `rest`.
fn first_name(rest: &Path) -> Option<String> {
    match rest.components().next()? {
        Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// The service of a `~/Library/CloudStorage` folder, named
/// `<Provider>-<account>` ("OneDrive-Personal").
fn provider_service(folder: &str) -> SyncService {
    let provider = folder.split('-').next().unwrap_or(folder);
    match provider {
        "Dropbox" => SyncService::Dropbox,
        "OneDrive" => SyncService::OneDrive,
        "GoogleDrive" => SyncService::GoogleDrive,
        "iCloudDrive" | "iCloud" => SyncService::ICloudDrive,
        other => SyncService::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(folder: &str, marks: &[&str]) -> Option<SyncService> {
        let home = Path::new("/Users/me");
        let one_drive = [PathBuf::from("/Users/me/Work Files")];
        let around = Around { home: Some(home), one_drive: &one_drive };
        let marks: Vec<PathBuf> = marks.iter().map(PathBuf::from).collect();
        service_of(Path::new(folder), &around, &|p| marks.iter().any(|m| m == p))
    }

    #[test]
    fn each_service_s_folders_are_told_by_their_path() {
        use SyncService::*;
        let icloud = "/Users/me/Library/Mobile Documents/com~apple~CloudDocs";
        let cases: &[(&str, &[&str], Option<SyncService>)] = &[
            (&format!("{icloud}/Deptheos"), &[], Some(ICloudDrive)),
            ("/Users/me/Library/Mobile Documents/iCloud~app/Deptheos", &[], Some(ICloudDrive)),
            ("/Users/me/iCloudDrive/Deptheos", &[], Some(ICloudDrive)),
            // Desktop & Documents Folders, on and off.
            ("/Users/me/Documents/Deptheos", &[&format!("{icloud}/Documents")], Some(ICloudDrive)),
            ("/Users/me/Desktop/Deptheos", &[&format!("{icloud}/Desktop")], Some(ICloudDrive)),
            ("/Users/me/Documents/Deptheos", &[&format!("{icloud}/Desktop")], None),
            ("/Users/me/Library/CloudStorage/Dropbox/Deptheos", &[], Some(Dropbox)),
            ("/Users/me/Library/CloudStorage/OneDrive-Personal/D", &[], Some(OneDrive)),
            (
                "/Users/me/Library/CloudStorage/GoogleDrive-me@x.com/My Drive",
                &[],
                Some(GoogleDrive),
            ),
            ("/Users/me/Library/CloudStorage/Box-Box/D", &[], Some(Other("Box".into()))),
            ("/Volumes/Big/Dropbox/Deptheos", &["/Volumes/Big/Dropbox/.dropbox"], Some(Dropbox)),
            ("/Users/me/OneDrive - Acme/Deptheos", &[], Some(OneDrive)),
            ("/Users/me/Work Files/Deptheos", &[], Some(OneDrive)),
            ("/Users/me/Google Drive/Deptheos", &[], Some(GoogleDrive)),
            // Not synced.
            ("/Users/me/Library/Application Support/Deptheos", &[], None),
            ("/Users/me/Pictures/Deptheos", &[], None),
            ("/Volumes/Photos/Deptheos", &[], None),
            ("/Users/me/Dropbox Old/Deptheos", &[], None),
        ];
        for (folder, marks, want) in cases {
            assert_eq!(&of(folder, marks), want, "{folder}");
        }
    }

    #[test]
    fn the_default_library_is_not_synced() {
        let default = crate::library::history::History::default_root().unwrap();
        assert_eq!(sync_service(&default), None, "{}", default.display());
    }
}
