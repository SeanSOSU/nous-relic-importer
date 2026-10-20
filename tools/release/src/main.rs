//! The release key, and each release's signatures and update feed (README, "Publishing a release").
//!
//!   nri-release keygen <secret key file> <public key file> [--password]
//!       A new release key. The secret half has no password unless --password asks for one; keep it out of the
//!       repo and back it up. The public half goes in update-key.pub, which every build trusts.
//!
//!   nri-release sign --key <secret key file> --base-url <url> --out <folder> [--public <public key file>]
//!                    [--version <version>] <platform>=<build> ...
//!       Signs each build as the version in Cargo.toml (refusing a build of any other version), copies it into
//!       the folder under its download name with its .minisig beside it, and writes the folder's latest.json,
//!       the feed the app reads. <url> is where the files will be downloadable, ending in /. For test builds,
//!       --public names the key they trust when it is not update-key.pub, and --version the version they are.
//!
//!   nri-release verify <download> [--public <public key file>]
//!       Checks a download against the .minisig beside it and the key update-key.pub holds, and prints what the
//!       signature names: program, version and platform.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use minisign::{KeyPair, PublicKey, PublicKeyBox, SecretKey, SignatureBox};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const USAGE: &str = "usage:\n  nri-release keygen <secret key file> <public key file> [--password]\n  \
    nri-release sign --key <secret key file> --base-url <url> --out <folder> [--public <public key file>] [--version <version>] \
    <platform>=<build> ...\n  nri-release verify <download> [--public <public key file>]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.first().map(String::as_str) {
        Some("keygen") => keygen(&args[1..]),
        Some("sign") => sign(&args[1..]),
        Some("verify") => verify(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(e) = done {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn keygen(args: &[String]) -> Result<(), String> {
    let password = args.iter().any(|a| a == "--password");
    let files: Vec<&String> = args.iter().filter(|a| *a != "--password").collect();
    let [secret, public] = files[..] else { return Err(USAGE.to_owned()) };
    if Path::new(secret).exists() {
        return Err(format!("{secret} already exists; a new key would orphan every build that trusts the old one"));
    }
    // An empty password stores the key as it is; None asks for one at the terminal.
    let KeyPair { pk, sk } = KeyPair::generate_encrypted_keypair(if password { None } else { Some(String::new()) }).map_err(|e| e.to_string())?;
    let secret_box = sk.to_box(Some("Nous Relic Importer release key: keep it secret and keep a backup")).map_err(|e| e.to_string())?;
    std::fs::write(secret, secret_box.to_string()).map_err(|e| format!("{secret}: {e}"))?;
    std::fs::write(public, pk.to_box().map_err(|e| e.to_string())?.to_string()).map_err(|e| format!("{public}: {e}"))?;
    println!("secret key: {secret}\npublic key: {public} ({})", pk.to_base64());
    Ok(())
}

/// The secret key, asking for its password only when it has one.
fn load_secret(path: &str) -> Result<SecretKey, String> {
    SecretKey::from_file(path, Some(String::new()))
        .or_else(|_| SecretKey::from_file(path, None))
        .map_err(|e| format!("{path}: {e}"))
}

/// The [package] version in the repo's Cargo.toml.
fn package_version() -> Result<String, String> {
    let manifest = std::fs::read_to_string(Path::new(REPO).join("Cargo.toml")).map_err(|e| e.to_string())?;
    let package = manifest.split("[package]").nth(1).ok_or("no [package] in Cargo.toml")?;
    let line = package.lines().find(|l| l.trim_start().starts_with("version")).ok_or("no package version in Cargo.toml")?;
    line.split('"').nth(1).map(str::to_owned).ok_or_else(|| "unreadable package version in Cargo.toml".to_owned())
}

/// The name a platform's build is downloaded as.
fn download_name(platform: &str) -> String {
    match platform {
        "windows-x86_64" => "nous-relic-importer.exe".into(),
        "linux-x86_64" => "nous-relic-importer".into(),
        other if other.starts_with("windows-") => format!("nous-relic-importer-{other}.exe"),
        other => format!("nous-relic-importer-{other}"),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The key the builds trust, update-key.pub unless `public` names another, and the file it came from.
fn trusted_key(public: Option<&String>) -> Result<(PublicKey, PathBuf), String> {
    let file = public.map_or_else(|| Path::new(REPO).join("update-key.pub"), PathBuf::from);
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let key = PublicKeyBox::from_string(&text).and_then(|b| b.into_public_key()).map_err(|e| e.to_string())?;
    Ok((key, file))
}

fn verify(args: &[String]) -> Result<(), String> {
    let (mut public, mut download) = (None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--public" => public = args.next(),
            file => download = Some(file),
        }
    }
    let download = download.ok_or(USAGE)?;
    let (trusted, _) = trusted_key(public)?;
    let signature = SignatureBox::from_file(format!("{download}.minisig")).map_err(|e| format!("{download}.minisig: {e}"))?;
    let bytes = std::fs::read(download).map_err(|e| format!("{download}: {e}"))?;
    minisign::verify(&trusted, &signature, Cursor::new(bytes), true, false, false).map_err(|e| format!("{download}: {e}"))?;
    println!("good signature: {}", signature.trusted_comment().map_err(|e| e.to_string())?);
    Ok(())
}

fn sign(args: &[String]) -> Result<(), String> {
    let (mut key, mut base_url, mut out, mut public, mut version) = (None, None, None, None, None);
    let mut builds = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--key" => key = args.next(),
            "--base-url" => base_url = args.next(),
            "--out" => out = args.next(),
            "--public" => public = args.next(),
            "--version" => version = args.next().cloned(),
            build => builds.push(build.split_once('=').ok_or(USAGE)?),
        }
    }
    let (Some(key), Some(base_url), Some(out)) = (key, base_url, out) else { return Err(USAGE.to_owned()) };
    if builds.is_empty() || !base_url.ends_with('/') {
        return Err(USAGE.to_owned());
    }

    let secret = load_secret(key)?;
    let signer = PublicKey::from_secret_key(&secret).map_err(|e| e.to_string())?;
    let (trusted, trusted_file) = trusted_key(public)?;
    if trusted.to_base64() != signer.to_base64() {
        return Err(format!("{key} is not the key {} holds: the builds would refuse these signatures", trusted_file.display()));
    }

    let version = match version {
        Some(version) => version,
        None => package_version()?,
    };
    let mark = format!("nous-relic-importer-version:{version}\0");
    let out = Path::new(out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut platforms = serde_json::Map::new();
    for (platform, build) in builds {
        let bytes = std::fs::read(build).map_err(|e| format!("{build}: {e}"))?;
        if !contains(&bytes, mark.as_bytes()) {
            return Err(format!("{build} is not a {version} build: build it again after setting the version"));
        }
        let name = download_name(platform);
        let comment = format!("nous-relic-importer {version} {platform}");
        let untrusted = format!("Nous Relic Importer {version}, {platform}: minisign -Vm {name} -p update-key.pub");
        let signature = minisign::sign(Some(&signer), &secret, Cursor::new(&bytes), Some(&comment), Some(&untrusted))
            .map_err(|e| e.to_string())?
            .to_string();
        std::fs::write(out.join(&name), &bytes).map_err(|e| e.to_string())?;
        std::fs::write(out.join(format!("{name}.minisig")), &signature).map_err(|e| e.to_string())?;
        platforms.insert(
            platform.to_owned(),
            serde_json::json!({ "url": format!("{base_url}{name}"), "size": bytes.len(), "signature": signature }),
        );
        println!("signed {build} as {comment}");
    }
    let feed = serde_json::json!({ "version": version, "platforms": platforms });
    let text = serde_json::to_string_pretty(&feed).map_err(|e| e.to_string())?;
    std::fs::write(out.join("latest.json"), text + "\n").map_err(|e| e.to_string())?;
    println!("wrote {}", out.join("latest.json").display());
    Ok(())
}
