//! The seedbox over SFTP: the only code that talks to it. Its host key is pinned:
//! a server presenting any key but `MELETE_SEEDBOX_HOST_KEY` is dropped during the handshake,
//! before the password is sent. There's no trusting a key on first use; `host_key` shows what a
//! server presents so an admin can check it and pin it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use russh::client;
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use serde::Serialize;

use crate::config;

/// How long connecting, the handshake and signing in may take. (A handshake cut off by it
/// leaves russh's own session task to end at its inactivity timeout; nothing was sent.)
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the whole connection test may take.
const TEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Why the seedbox couldn't be reached or read. None of them quotes the password.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The server presented another key than the pinned one: possibly an impostor.
    HostKey { presented: String },
    /// The user name and password were refused.
    Rejected,
    /// No connection, or signing in didn't finish: a network error or a timeout.
    Unreachable(String),
    /// Signed in, but SFTP failed: no SFTP subsystem, or a folder can't be listed.
    Sftp(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::HostKey { presented } => write!(
                f,
                "the seedbox presented the host key {presented}, not the pinned one; nothing was sent. If the seedbox really changed its key, check the new one with your provider and update MELETE_SEEDBOX_HOST_KEY"
            ),
            Error::Rejected => f.write_str("the seedbox refused the user name and password"),
            Error::Unreachable(why) => write!(f, "can't reach the seedbox: {why}"),
            Error::Sftp(why) => write!(f, "the seedbox's SFTP: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// A key's fingerprint as OpenSSH writes it ("SHA256:..."); certificates aren't accepted.
fn fingerprint(key: &PublicKeyOrCertificate) -> String {
    match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => {
            key.fingerprint(HashAlg::Sha256).to_string()
        }
        PublicKeyOrCertificate::Certificate(_) => "a certificate".to_owned(),
    }
}

/// Accepts the server's key only when it's the pinned one, and notes what it presented.
struct Pin {
    pinned: Option<String>,
    presented: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Pin {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let presented = fingerprint(key);
        let accepted = self.pinned.as_deref() == Some(presented.as_str());
        *self
            .presented
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(presented);
        Ok(accepted)
    }
}

fn ssh_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(120)),
        ..client::Config::default()
    })
}

/// Connects without credentials and returns the key the server presents; the connection is
/// dropped right after.
pub async fn host_key(host: &str, port: u16) -> Result<String, Error> {
    let presented = Arc::new(Mutex::new(None));
    let pin = Pin {
        pinned: None,
        presented: presented.clone(),
    };
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(ssh_config(), (host, port), pin),
    )
    .await;
    if let Some(key) = presented
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        return Ok(key);
    }
    Err(match connected {
        Err(_) => Error::Unreachable("timed out".into()),
        Ok(Err(err)) => Error::Unreachable(err.to_string()),
        Ok(Ok(_)) => Error::Unreachable("it presented no host key".into()),
    })
}

/// The seedbox as configured: its address, pinned host key, sign-in and folders. It reads
/// only; nothing here writes to or deletes from the seedbox.
pub struct Seedbox {
    settings: config::Seedbox,
}

/// A signed-in SFTP session.
pub struct Connection {
    /// The SFTP session, for reading the seedbox's files.
    pub sftp: SftpSession,
    // Kept so the SSH connection outlives the SFTP session.
    _handle: client::Handle<Pin>,
}

/// What a connection test found.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct TestReport {
    /// The pinned host key it connected with, as OpenSSH writes it.
    pub host_key: String,
    /// The SFTP folder Deluge's finished torrents are in.
    pub folder: String,
    /// How many entries that folder holds.
    pub entries: usize,
    /// Whether the folders Deluge keeps completed downloads in are where MELETE_SEEDBOX_PATHS
    /// says, as SFTP sees them.
    pub completed: Vec<PathCheck>,
}

/// Whether one of Deluge's folders for completed downloads is where SFTP expects it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PathCheck {
    /// The folder as Deluge names it.
    pub deluge_path: String,
    /// None when it's outside Deluge's folder for finished torrents.
    pub sftp_path: Option<String>,
    /// Whether SFTP finds a folder at `sftp_path`; false when that is None.
    pub found: bool,
}

impl Seedbox {
    /// The seedbox with these settings; nothing connects until [`connect`](Self::connect).
    pub fn new(settings: config::Seedbox) -> Self {
        Self { settings }
    }

    /// Connects, checks the host key against the pinned one before anything is sent, signs in
    /// with the password, and opens an SFTP session.
    ///
    /// # Errors
    ///
    /// [`Error::HostKey`] when the server presents another key, [`Error::Rejected`] when the
    /// user name and password are refused, [`Error::Unreachable`] when connecting or signing in
    /// fails or takes over 30 seconds, and [`Error::Sftp`] when the SFTP session can't be opened.
    pub async fn connect(&self) -> Result<Connection, Error> {
        let presented = Arc::new(Mutex::new(None));
        let pin = Pin {
            pinned: Some(self.settings.host_key.clone()),
            presented: presented.clone(),
        };
        let address = (self.settings.host.as_str(), self.settings.port);
        let connected =
            tokio::time::timeout(CONNECT_TIMEOUT, client::connect(ssh_config(), address, pin))
                .await;
        let mut handle = match connected {
            Err(_) => return Err(Error::Unreachable("timed out".into())),
            Ok(Ok(handle)) => handle,
            Ok(Err(err)) => {
                let presented = presented
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                return Err(match presented {
                    Some(key) if key != self.settings.host_key => Error::HostKey { presented: key },
                    _ => Error::Unreachable(err.to_string()),
                });
            }
        };
        let signed_in = tokio::time::timeout(
            CONNECT_TIMEOUT,
            handle
                .authenticate_password(self.settings.user.clone(), self.settings.password.expose()),
        )
        .await
        .map_err(|_| Error::Unreachable("signing in timed out".into()))?
        .map_err(|err| Error::Unreachable(err.to_string()))?;
        if !signed_in.success() {
            return Err(Error::Rejected);
        }
        let sftp_error = |err: russh::Error| Error::Sftp(err.to_string());
        let channel = handle.channel_open_session().await.map_err(sftp_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(sftp_error)?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|err| Error::Sftp(err.to_string()))?;
        Ok(Connection {
            sftp,
            _handle: handle,
        })
    }

    /// Where a folder Deluge names (its save path) is as SFTP sees it; None outside Deluge's
    /// folder for finished torrents, which is all Melete may fetch from.
    pub fn sftp_path(&self, deluge_path: &str) -> Option<String> {
        let (deluge, sftp) = (&self.settings.deluge_dir, &self.settings.sftp_dir);
        let rest = deluge_path
            .trim_end_matches('/')
            .strip_prefix(deluge.as_str())?;
        if !(rest.is_empty() || rest.starts_with('/')) || rest.split('/').any(|part| part == "..") {
            return None;
        }
        Some(match sftp.as_str() {
            "/" if rest.is_empty() => "/".to_owned(),
            "/" => rest.to_owned(),
            _ => format!("{sftp}{rest}"),
        })
    }

    /// Connects, signs in, lists the folder for finished torrents, and checks that the
    /// folders Deluge named for completed downloads are there; all within two minutes.
    pub async fn test(&self, deluge_paths: &[String]) -> Result<TestReport, Error> {
        tokio::time::timeout(TEST_TIMEOUT, self.test_untimed(deluge_paths))
            .await
            .map_err(|_| Error::Unreachable("the test took more than two minutes".into()))?
    }

    async fn test_untimed(&self, deluge_paths: &[String]) -> Result<TestReport, Error> {
        let connection = self.connect().await?;
        let folder = self.settings.sftp_dir.clone();
        let entries = connection
            .sftp
            .read_dir(&folder)
            .await
            .map_err(|err| Error::Sftp(format!("can't list {folder}: {err}")))?
            .count();
        let mut completed = Vec::new();
        for deluge_path in deluge_paths {
            let sftp_path = self.sftp_path(deluge_path);
            let found = match &sftp_path {
                Some(path) => connection
                    .sftp
                    .metadata(path)
                    .await
                    .is_ok_and(|m| m.is_dir()),
                None => false,
            };
            completed.push(PathCheck {
                deluge_path: deluge_path.clone(),
                sftp_path,
                found,
            });
        }
        Ok(TestReport {
            host_key: self.settings.host_key.clone(),
            folder,
            entries,
            completed,
        })
    }
}

/// An SSH server with SFTP over a folder, for tests: user "user", password "pw", and an
/// Ed25519 host key made from a fixed seed. It notes every password it's offered.
#[cfg(test)]
pub mod testing {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use russh::keys::ssh_key::private::Ed25519Keypair;
    use russh::keys::{HashAlg, PrivateKey};
    use russh::server::{Auth, Msg, Server as _, Session};
    use russh::{Channel, ChannelId};
    use russh_sftp::protocol::{
        Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
    };

    pub struct Running {
        pub port: u16,
        pub fingerprint: String,
        /// Passwords offered, in order.
        pub passwords: Arc<Mutex<Vec<String>>>,
    }

    pub fn host_key(seed: u8) -> PrivateKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
    }

    pub async fn start(root: &Path) -> Running {
        let key = host_key(7);
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(russh::server::Config {
            keys: vec![key],
            auth_rejection_time: std::time::Duration::from_millis(10),
            auth_rejection_time_initial: Some(std::time::Duration::ZERO),
            ..Default::default()
        });
        let passwords = Arc::new(Mutex::new(Vec::new()));
        let mut server = Server {
            root: root.to_path_buf(),
            passwords: passwords.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { server.run_on_socket(config, &listener).await });
        Running {
            port,
            fingerprint,
            passwords,
        }
    }

    #[derive(Clone)]
    struct Server {
        root: PathBuf,
        passwords: Arc<Mutex<Vec<String>>>,
    }

    impl russh::server::Server for Server {
        type Handler = Connection;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Connection {
            Connection {
                server: self.clone(),
                channels: HashMap::new(),
            }
        }
    }

    struct Connection {
        server: Server,
        channels: HashMap<ChannelId, Channel<Msg>>,
    }

    impl russh::server::Handler for Connection {
        type Error = russh::Error;

        async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
            self.server
                .passwords
                .lock()
                .unwrap()
                .push(password.to_owned());
            Ok(match (user, password) {
                ("user", "pw") => Auth::Accept,
                _ => Auth::Reject {
                    proceed_with_methods: None,
                    partial_success: false,
                },
            })
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<Msg>,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.channels.insert(channel.id(), channel);
            reply.accept().await;
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            id: ChannelId,
            name: &str,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            match (name, self.channels.remove(&id)) {
                ("sftp", Some(channel)) => {
                    session.channel_success(id)?;
                    let sftp = Sftp {
                        root: self.server.root.clone(),
                        dirs: HashMap::new(),
                        files: HashMap::new(),
                        next: 0,
                    };
                    russh_sftp::server::run(channel.into_stream(), sftp).await;
                }
                _ => session.channel_failure(id)?,
            }
            Ok(())
        }
    }

    /// SFTP over a folder; paths are taken as relative to it, and ".." is refused.
    struct Sftp {
        root: PathBuf,
        /// Open directory handles: their path, and whether they were listed already.
        dirs: HashMap<String, (String, bool)>,
        /// Open files by handle.
        files: HashMap<String, PathBuf>,
        next: u64,
    }

    impl Sftp {
        fn local(&self, path: &str) -> Result<PathBuf, StatusCode> {
            if path.split('/').any(|part| part == "..") {
                return Err(StatusCode::PermissionDenied);
            }
            Ok(self.root.join(path.trim_start_matches('/')))
        }
    }

    impl russh_sftp::server::Handler for Sftp {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
            let path = if path == "." { "/".to_owned() } else { path };
            Ok(Name {
                id,
                files: vec![File::dummy(path)],
            })
        }

        async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            let metadata =
                std::fs::metadata(self.local(&path)?).map_err(|_| StatusCode::NoSuchFile)?;
            Ok(Attrs {
                id,
                attrs: FileAttributes::from(&metadata),
            })
        }

        /// Links are reported as links, with their real mode: FileAttributes::from calls
        /// anything that isn't a folder a regular file.
        async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::symlink_metadata(self.local(&path)?)
                .map_err(|_| StatusCode::NoSuchFile)?;
            let mut attrs = FileAttributes::from(&metadata);
            attrs.permissions = Some(metadata.mode());
            Ok(Attrs { id, attrs })
        }

        async fn open(
            &mut self,
            id: u32,
            filename: String,
            _flags: OpenFlags,
            _attrs: FileAttributes,
        ) -> Result<Handle, Self::Error> {
            let path = self.local(&filename)?;
            if !path.is_file() {
                return Err(StatusCode::NoSuchFile);
            }
            self.next += 1;
            let handle = format!("file{}", self.next);
            self.files.insert(handle.clone(), path);
            Ok(Handle { id, handle })
        }

        async fn read(
            &mut self,
            id: u32,
            handle: String,
            offset: u64,
            len: u32,
        ) -> Result<Data, Self::Error> {
            use std::io::{Read, Seek, SeekFrom};
            let path = self.files.get(&handle).ok_or(StatusCode::Failure)?;
            let mut file = std::fs::File::open(path).map_err(|_| StatusCode::Failure)?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|_| StatusCode::Failure)?;
            let mut data = vec![0; len as usize];
            let read = file.read(&mut data).map_err(|_| StatusCode::Failure)?;
            if read == 0 {
                return Err(StatusCode::Eof);
            }
            data.truncate(read);
            Ok(Data { id, data })
        }

        async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
            if !self.local(&path)?.is_dir() {
                return Err(StatusCode::NoSuchFile);
            }
            self.next += 1;
            let handle = format!("dir{}", self.next);
            self.dirs.insert(handle.clone(), (path, false));
            Ok(Handle { id, handle })
        }

        async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
            let (path, listed) = self.dirs.get(&handle).cloned().ok_or(StatusCode::Failure)?;
            if listed {
                return Err(StatusCode::Eof);
            }
            self.dirs.insert(handle, (path.clone(), true));
            let mut files = Vec::new();
            for entry in
                std::fs::read_dir(self.local(&path)?).map_err(|_| StatusCode::NoSuchFile)?
            {
                let entry = entry.map_err(|_| StatusCode::Failure)?;
                let metadata = entry.metadata().map_err(|_| StatusCode::Failure)?;
                files.push(File::new(
                    entry.file_name().to_string_lossy(),
                    FileAttributes::from(&metadata),
                ));
            }
            if files.is_empty() {
                return Err(StatusCode::Eof);
            }
            Ok(Name { id, files })
        }

        async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
            self.dirs.remove(&handle);
            self.files.remove(&handle);
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".into(),
                language_tag: "en-US".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Secret;

    fn settings(port: u16, host_key: &str, password: &str) -> config::Seedbox {
        config::Seedbox {
            host: "127.0.0.1".into(),
            port,
            user: "user".into(),
            password: Secret::for_tests(password),
            host_key: host_key.into(),
            deluge_dir: "/home/user/Completed".into(),
            sftp_dir: "/Completed".into(),
        }
    }

    #[test]
    fn maps_delugess_folders_to_sftp_and_nothing_outside() {
        let seedbox = Seedbox::new(settings(22, "SHA256:x", "pw"));
        let map = |path: &str| seedbox.sftp_path(path);
        assert_eq!(map("/home/user/Completed").as_deref(), Some("/Completed"));
        assert_eq!(map("/home/user/Completed/").as_deref(), Some("/Completed"));
        assert_eq!(
            map("/home/user/Completed/movies").as_deref(),
            Some("/Completed/movies")
        );
        assert_eq!(map("/home/user/CompletedOther"), None);
        assert_eq!(map("/home/user/Downloads"), None);
        assert_eq!(map("/home/user/Completed/../.ssh"), None);
        let mut root = settings(22, "SHA256:x", "pw");
        root.sftp_dir = "/".into();
        let at_root = Seedbox::new(root);
        assert_eq!(
            at_root.sftp_path("/home/user/Completed/a").as_deref(),
            Some("/a")
        );
        assert_eq!(
            at_root.sftp_path("/home/user/Completed").as_deref(),
            Some("/")
        );
    }

    #[tokio::test]
    async fn connects_only_to_the_pinned_key() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Completed/Some.Movie")).unwrap();
        std::fs::write(root.path().join("Completed/Some.Movie/movie.mkv"), b"x").unwrap();
        let server = testing::start(root.path()).await;

        // Learning the key sends nothing.
        assert_eq!(
            host_key("127.0.0.1", server.port).await.unwrap(),
            server.fingerprint
        );
        assert!(server.passwords.lock().unwrap().is_empty());

        let pinned = Seedbox::new(settings(server.port, &server.fingerprint, "pw"));
        let report = pinned
            .test(&[
                "/home/user/Completed".into(),
                "/home/user/Completed/Gone".into(),
                "/elsewhere".into(),
            ])
            .await
            .unwrap();
        assert_eq!((report.folder.as_str(), report.entries), ("/Completed", 1));
        let found: Vec<(Option<&str>, bool)> = report
            .completed
            .iter()
            .map(|c| (c.sftp_path.as_deref(), c.found))
            .collect();
        assert_eq!(
            found,
            [
                (Some("/Completed"), true),
                (Some("/Completed/Gone"), false),
                (None, false)
            ]
        );

        // Another key: refused during the handshake, before the password goes anywhere.
        let before = server.passwords.lock().unwrap().len();
        let other = testing::host_key(8)
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();
        let impostor_check = Seedbox::new(settings(server.port, &other, "pw"));
        let err = impostor_check.test(&[]).await.unwrap_err();
        assert_eq!(
            err,
            Error::HostKey {
                presented: server.fingerprint.clone()
            }
        );
        assert!(err.to_string().contains("nothing was sent"));
        assert_eq!(
            server.passwords.lock().unwrap().len(),
            before,
            "no password was offered"
        );

        let wrong = Seedbox::new(settings(server.port, &server.fingerprint, "nope"));
        let err = wrong.test(&[]).await.unwrap_err();
        assert_eq!(err, Error::Rejected);
        assert!(!err.to_string().contains("nope"));
    }

    #[tokio::test]
    async fn reports_an_unreachable_seedbox() {
        let gone = Seedbox::new(settings(9, "SHA256:x", "pw"));
        assert!(matches!(
            gone.test(&[]).await.unwrap_err(),
            Error::Unreachable(_)
        ));
        assert!(matches!(
            host_key("127.0.0.1", 9).await.unwrap_err(),
            Error::Unreachable(_)
        ));
    }
}
