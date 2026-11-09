//! Faux serveur de publications des outils de la vidéo (SPEC §7.17) : yt-dlp et sa liste
//! d'empreintes signée par une clé de test, archive de ffmpeg fabriquée ici. Sans Internet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aws_lc_rs::signature::{KeyPair as _, RsaKeyPair};
use sha2::{Digest as _, Sha256};

use super::{Server, reply, serve};
use crate::media::openpgp::{self, PublicKey};
use crate::media::tools::{Sources, exe, ffmpeg_asset, hex, ytdlp_asset};

/// Un faux GitHub : chemin → contenu.
pub struct FakeRelease {
    pub files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    server: Server,
    pair: RsaKeyPair,
    key: PublicKey,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

impl Default for FakeRelease {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeRelease {
    pub fn new() -> FakeRelease {
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let served = Arc::clone(&files);
        let server = serve(
            move |request| match served.lock().unwrap().get(&request.path) {
                Some(body) => reply("200 OK", &[], body),
                None => reply("404 Not Found", &[], b""),
            },
        );
        let pair = RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let components = aws_lc_rs::rsa::PublicKeyComponents::<Vec<u8>>::from(pair.public_key());
        let key = PublicKey::from_components(&components.n, &components.e, 1_700_000_000);
        FakeRelease {
            files,
            server,
            pair,
            key,
        }
    }

    /// Chemins demandés jusqu'ici.
    pub fn requested(&self) -> Vec<String> {
        self.server.requests().into_iter().map(|r| r.path).collect()
    }

    pub fn put(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), body);
    }

    /// Publie yt-dlp `version` (contenu `program`), sa liste d'empreintes signée par
    /// `signer` (la clé de ce serveur si `None`), et en fait la dernière publication.
    pub fn publish_ytdlp(&self, version: &str, program: &[u8], signer: Option<&FakeRelease>) {
        let asset = ytdlp_asset().unwrap();
        let sums = format!(
            "{}  yt-dlp\n{}  {asset}\n",
            sha256_hex(b"autre"),
            sha256_hex(program)
        );
        let signer = signer.unwrap_or(self);
        let sig = openpgp::sign_for_tests(&signer.pair, &signer.key, sums.as_bytes());
        self.put(&format!("/dl/{version}/{asset}"), program.to_vec());
        self.put(&format!("/dl/{version}/SHA2-256SUMS"), sums.into_bytes());
        self.put(&format!("/dl/{version}/SHA2-256SUMS.sig"), sig);
        self.put(
            "/api/latest",
            format!(r#"{{"tag_name": "{version}", "assets": []}}"#).into_bytes(),
        );
    }

    /// Publie une archive de ffmpeg qui contient ces fichiers (chemin → contenu).
    pub fn publish_ffmpeg(&self, entries: &[(&str, &[u8])]) {
        let asset = ffmpeg_asset().unwrap();
        let archive = make_archive(entries);
        self.put(
            "/ffmpeg/checksums.sha256",
            format!("{}  {asset}\n", sha256_hex(&archive)).into_bytes(),
        );
        self.put(&format!("/ffmpeg/{asset}"), archive);
    }

    /// Publie une archive de ffmpeg complète : `bin/ffmpeg`, `bin/ffprobe` (contenus
    /// « faux ffmpeg », « faux ffprobe »), et d'autres fichiers à ignorer.
    pub fn publish_default_ffmpeg(&self) {
        let top = "ffmpeg-n8.1-latest-gpl-8.1";
        let entries = [
            (format!("{top}/LICENSE.txt"), b"GPL".to_vec()),
            (
                format!("{top}/bin/{}", exe("ffmpeg")),
                b"faux ffmpeg".to_vec(),
            ),
            (
                format!("{top}/bin/{}", exe("ffprobe")),
                b"faux ffprobe".to_vec(),
            ),
            (
                format!("{top}/bin/{}", exe("ffplay")),
                b"faux ffplay".to_vec(),
            ),
        ];
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        self.publish_ffmpeg(&refs);
    }

    /// Les sources du moteur, tournées vers ce serveur.
    pub fn sources(&self) -> Sources {
        Sources {
            ytdlp_latest: self.server.url("/api/latest"),
            ytdlp_download: self.server.url("/dl/"),
            ytdlp_key: self.key.clone(),
            ffmpeg_download: self.server.url("/ffmpeg/"),
            system_ffmpeg: false,
            local_ytdlp: None,
        }
    }
}

/// Archive au format de BtbN pour cette plateforme : zip sous Windows, tar.xz sous Linux.
pub fn make_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::io::Write as _;
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }
    #[cfg(target_os = "linux")]
    {
        let xz = xz2::write::XzEncoder::new(Vec::new(), 6);
        let mut tar = tar::Builder::new(xz);
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, *body).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = entries;
        Vec::new()
    }
}

/// Un faux yt-dlp (script), dans `dir` : il note ses arguments dans `args.txt` (un par
/// ligne sous Linux), écrit `stdout` sur sa sortie et `stderr` sur sa sortie d'erreur,
/// attend `sleep_s` secondes, puis rend `code`.
pub fn fake_ytdlp(
    dir: &std::path::Path,
    stdout: &str,
    stderr: &str,
    code: i32,
    sleep_s: u32,
) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("out.txt"), stdout).unwrap();
    std::fs::write(dir.join("err.txt"), stderr).unwrap();
    #[cfg(windows)]
    {
        let script = dir.join("yt-dlp.cmd");
        let wait = if sleep_s > 0 {
            format!("ping -n {} 127.0.0.1 >nul\r\n", sleep_s + 1)
        } else {
            String::new()
        };
        std::fs::write(
            &script,
            format!(
                "@echo off\r\necho %*>\"%~dp0args.txt\"\r\ntype \"%~dp0out.txt\"\r\n\
                 type \"%~dp0err.txt\" 1>&2\r\n{wait}exit /b {code}\r\n"
            ),
        )
        .unwrap();
        script
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let script = dir.join("yt-dlp");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\n' \"$@\" > \"$d/args.txt\"\n\
                 cat \"$d/out.txt\"\ncat \"$d/err.txt\" >&2\nsleep {sleep_s}\nexit {code}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }
}
