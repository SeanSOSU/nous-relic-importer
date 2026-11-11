// Downloads the game tables the exporter reads (relic sets, stats, names) so they are compiled in. Copied from
// Reliquary Archiver's build.rs (MIT, https://github.com/IceDynamix/reliquary-archiver) without its Windows
// resource block, which lives in the app's own build.rs; keep the downloads in step when
// tools/update-reliquary.ps1 reports that the archiver's build.rs changed. Unlike the archiver's, it downloads them
// at the TurnBasedGameData commit gamedata.rev names, not at whatever main is: the same source always builds the same
// tables, and moving that file (tools/patch-check.ps1 does when the game's data moves) is what refreshes them. GitLab
// refuses unauthenticated raw downloads past a rate limit (429), so each file falls back to the GitHub mirror, which
// has the same commits.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::path::Path;
use std::sync::LazyLock;

use reliquary::resource::excel::{
    AvatarConfigMap, AvatarSkillTreeConfigMap, EquipmentConfigMap, ItemConfigMap, MultiplePathAvatarConfigMap, RelicConfigMap,
    RelicMainAffixConfigMap, RelicSetConfigMap, RelicSubAffixConfigMap,
};
use reliquary::resource::{ResourceMap, TextMapEntry};
use ureq::serde::Serialize;
use ureq::serde::de::DeserializeOwned;

const SOURCES: [&str; 2] = [
    "https://gitlab.com/Dimbreath/turnbasedgamedata/-/raw",
    "https://raw.githubusercontent.com/DimbreathBot/TurnBasedGameData",
];

static COMMIT: LazyLock<String> = LazyLock::new(|| {
    let path = Path::new(&env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("gamedata.rev");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let commit = text.split_whitespace().next().unwrap_or_default();
    assert!(
        commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "{} must start with a full TurnBasedGameData commit id",
        path.display()
    );
    commit.to_string()
});

macro_rules! download_config {
    ($t:ty, $ex:expr, [$($url:expr),+ $(,)?]) => {
        write_to_out(
            {
                let mut merged = <$t>::new_empty();
                $(
                    let value = download_as_json::<$t>($url);
                    $ex(&value);
                    merged.0.extend(value.0);
                )+
                merged
            },
            <$t>::get_json_name(),
        )
    };

    ($t:ty, [$($url:expr),+ $(,)?]) => {
        download_config!($t, |_| {}, [$($url),+]);
    };

    ($t:ty, $ex:expr) => {
        download_config!($t, $ex, [&resource_path::<$t>()]);
    };

    ($t:ty) => {
        download_config!($t, |_| {}, [&resource_path::<$t>()]);
    };
}

macro_rules! download_config_and_store_text_hashes {
    ($t:ty, $field:ident, $hashes:ident, $urls:tt) => {
        download_config!(
            $t,
            |value: &$t| {
                for cfg in value.0.iter() {
                    $hashes.insert(cfg.$field);
                }
            },
            $urls
        );
    };

    ($t:ty, $field:ident, $hashes:ident) => {
        download_config_and_store_text_hashes!($t, $field, $hashes, [&resource_path::<$t>()]);
    };
}

macro_rules! download_config_and_store_partial_text_hashes {
    ($t:ty, $field:ident, $hashes:ident) => {
        download_config!($t, |value: &$t| {
            for cfg in value.0.iter() {
                if let Some(hash) = cfg.$field {
                    $hashes.insert(hash);
                }
            }
        });
    };
}

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=gamedata.rev");

    // the text map is really, REALLY large (>25MB), so we're optimizing by only
    // keeping the entries used from relevant config files where the strings are required
    // for the export
    let mut text_hashes: HashSet<TextMapEntry> = HashSet::new();

    download_config_and_store_text_hashes!(AvatarConfigMap, AvatarName, text_hashes, [
        &resource_path::<AvatarConfigMap>(),
        &resource_path_of("AvatarConfigLD.json"),
    ]);
    download_config_and_store_text_hashes!(EquipmentConfigMap, EquipmentName, text_hashes);
    download_config_and_store_text_hashes!(RelicSetConfigMap, SetName, text_hashes);
    download_config_and_store_partial_text_hashes!(ItemConfigMap, ItemName, text_hashes);

    download_config!(AvatarSkillTreeConfigMap, [
        &resource_path::<AvatarSkillTreeConfigMap>(),
        &resource_path_of("AvatarSkillTreeConfigLD.json"),
    ]);
    download_config!(MultiplePathAvatarConfigMap);
    download_config!(RelicConfigMap);
    download_config!(RelicMainAffixConfigMap);
    download_config!(RelicSubAffixConfigMap);

    save_text_map(&text_hashes, "EN");
}

fn save_text_map(hashes: &HashSet<TextMapEntry>, language: &str) {
    let hashes: HashSet<String> = hashes.iter().map(|k| k.Hash.to_string()).collect();

    let file_name = format!("TextMap{language}.json");

    let text_map: HashMap<String, String> = download_as_json::<HashMap<String, String>>(&format!("TextMap/{file_name}"))
        .into_iter()
        .filter(|(k, _)| hashes.contains(k))
        .collect();

    write_to_out(text_map, &file_name);
}

fn resource_path<T: ResourceMap>() -> String {
    resource_path_of(T::get_json_name())
}

fn resource_path_of(name: &str) -> String {
    format!("ExcelOutput/{name}")
}

fn download_as_json<T: DeserializeOwned>(path: &str) -> T {
    let mut failures = Vec::new();
    for source in SOURCES {
        let url = format!("{source}/{}/{path}", *COMMIT);
        match ureq::get(&url).call() {
            Ok(response) => match response.into_json() {
                Ok(value) => {
                    if !failures.is_empty() {
                        println!("cargo:warning={}; took {url} instead", failures.join("; "));
                    }
                    return value;
                }
                Err(e) => failures.push(format!("{url}: {e}")),
            },
            Err(e) => failures.push(e.to_string()),
        }
    }
    panic!("Could not download {path}:\n{}", failures.join("\n"))
}

fn write_to_out<T: DeserializeOwned + Serialize>(value: T, file_name: &str) {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join(file_name);

    let mut file = File::create(out_path).unwrap();

    ureq::serde_json::to_writer(&mut file, &value).unwrap();
}
