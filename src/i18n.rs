//! The app's languages, the ones StarRailSim speaks: English, Japanese, Korean and Simplified Chinese. Settings
//! picks one, or Automatic, which follows the system's display language. The English text is the key, as on the
//! site, so anything without a translation shows in English. Game terms follow the game's own (遺物, 유물, 遗器).

use std::sync::atomic::{AtomicU8, Ordering};

use serde_json::{Map, Value};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    En,
    Ja,
    Ko,
    Zh,
}

impl Lang {
    const ALL: [Lang; 4] = [Lang::En, Lang::Ja, Lang::Ko, Lang::Zh];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ja => "ja",
            Lang::Ko => "ko",
            Lang::Zh => "zh-CN",
        }
    }

    /// What the language calls itself, for the picker.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ja => "日本語",
            Lang::Ko => "한국어",
            Lang::Zh => "简体中文",
        }
    }

    /// A language tag or locale ("ja-JP", "ko_KR.UTF-8", "zh-Hans-CN", "zh-CN") as one of ours.
    fn from_tag(tag: &str) -> Option<Lang> {
        let primary = tag.split(['-', '_', '.', '@']).next()?.to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code().split('-').next() == Some(primary.as_str()))
    }
}

const SETTING: &str = "language";
const AUTO: &str = "auto";
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The reader's pick: a language code, or "auto".
pub fn choice() -> String {
    crate::settings::get_text(SETTING, AUTO)
}

/// Settles this run's language from the setting, or from the system on Automatic.
pub fn apply() {
    let lang = Lang::from_tag(&choice()).unwrap_or_else(system);
    CURRENT.store(Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0) as u8, Ordering::Relaxed);
}

/// The reader picked a language ("auto" or a code); unknown codes count as Automatic.
pub fn set_choice(code: &str) {
    let code = if Lang::from_tag(code).is_some() { code } else { AUTO };
    crate::settings::set_text(SETTING, code);
    apply();
}

pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize]
}

/// The system's display language when it is one of ours, else English.
pub fn system() -> Lang {
    #[cfg(windows)]
    {
        // The UI language's primary id: LANG_JAPANESE, LANG_KOREAN, LANG_CHINESE (Simplified and Traditional).
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff;
        match id {
            0x11 => Lang::Ja,
            0x12 => Lang::Ko,
            0x04 => Lang::Zh,
            _ => Lang::En,
        }
    }
    #[cfg(not(windows))]
    {
        // As gettext reads them: LANGUAGE's list first, then LC_ALL, LC_MESSAGES, LANG; "C" and empty say nothing.
        ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .flat_map(|value| value.split(':').map(str::to_owned).collect::<Vec<_>>())
            .find_map(|tag| Lang::from_tag(&tag))
            .unwrap_or(Lang::En)
    }
}

/// `english` in the current language.
pub fn t(english: &'static str) -> &'static str {
    let column = match current() {
        Lang::En => return english,
        Lang::Ja => 0,
        Lang::Ko => 1,
        Lang::Zh => 2,
    };
    TABLE.iter().find(|(key, _)| *key == english).map_or(english, |(_, row)| row[column])
}

/// `t`, then each `{name}` filled in.
pub fn tf(english: &'static str, args: &[(&str, &str)]) -> String {
    args.iter().fold(t(english).to_owned(), |text, (name, value)| text.replace(&format!("{{{name}}}"), value))
}

/// What the window page words itself, in the current language, keyed by its English.
pub fn page_strings() -> Value {
    Value::Object(PAGE.iter().map(|key| ((*key).to_owned(), Value::from(t(key)))).collect::<Map<_, _>>())
}

/// The page's own words: everything it shows that the state does not carry.
const PAGE: &[&str] = &[
    "relics",
    "light cones",
    "characters",
    "Top relic sets",
    "Open StarRailSim",
    "Closing this window keeps sync running in the background.",
    "Settings",
    "Language",
    "Automatic",
    "Install updates automatically",
    "Off: the site keeps what it last got.",
    "Available once installed: run the downloaded file without --portable.",
    "Saved file",
    "Uninstall",
    "Back",
    "Last updated {ago}",
    "Saved {ago}",
    "just now",
    "Last checked {ago}.",
    "Uninstall Nous Relic Importer? It stops syncing and removes itself from this computer.",
    "Installing the update",
    "Nous Relic Importer is not running",
    "The new version opens its own window in a moment, once you have entered your password.",
    "Start it from your app menu, then reload this page.",
];

/// English, then Japanese, Korean and Simplified Chinese.
const TABLE: &[(&str, [&str; 3])] = &[
    // The window and the tray.
    ("Starting…", ["起動中…", "시작하는 중…", "正在启动…"]),
    ("Not working", ["動作していません", "작동하지 않음", "无法运行"]),
    (
        "{why}. Quit and start it again; if it keeps happening, the log is in {logs}.",
        [
            "{why}。終了してからもう一度起動してください。繰り返し起こる場合、ログは {logs} にあります。",
            "{why}. 종료한 뒤 다시 시작하세요. 계속 발생하면 로그는 {logs}에 있습니다.",
            "{why}。请退出后重新启动；如果问题持续，日志位于 {logs}。",
        ],
    ),
    ("Waiting for the game", ["ゲームを待っています", "게임을 기다리는 중", "正在等待游戏"]),
    (
        "Start Honkai: Star Rail and click to start. If you are already in game, log out and back in.",
        [
            "崩壊：スターレイルを起動し、スタート画面でクリックして開始します。すでにゲーム内にいる場合は、一度ログアウトしてからログインし直してください。",
            "붕괴: 스타레일을 실행하고 시작 화면에서 클릭해 시작하세요. 이미 게임에 접속해 있다면 로그아웃 후 다시 로그인하세요.",
            "启动崩坏：星穹铁道并点击开始。如果你已经在游戏中，请先登出再重新登录。",
        ],
    ),
    ("Game found", ["ゲームを検出しました", "게임을 찾았습니다", "已找到游戏"]),
    (
        "Log in and your account loads here.",
        ["ログインすると、アカウントがここに読み込まれます。", "로그인하면 계정이 여기에 불러와집니다.", "登录后，你的账号会载入这里。"],
    ),
    ("Loading your account…", ["アカウントを読み込み中…", "계정을 불러오는 중…", "正在载入你的账号…"]),
    ("Log out and back in", ["ログアウトしてログインし直してください", "로그아웃 후 다시 로그인하세요", "请登出后重新登录"]),
    (
        "The game was already running when sync started, so it missed the login. Log out to the start screen and back in. If the game just updated, this app may need an update too.",
        [
            "同期を始めたときにはゲームがすでに起動していたため、ログインを読み取れませんでした。スタート画面までログアウトしてから、もう一度ログインしてください。ゲームが更新された直後の場合は、このアプリの更新も必要かもしれません。",
            "동기화가 시작될 때 게임이 이미 실행 중이어서 로그인을 놓쳤습니다. 시작 화면으로 로그아웃한 뒤 다시 로그인하세요. 게임이 방금 업데이트되었다면 이 앱도 업데이트가 필요할 수 있습니다.",
            "同步开始时游戏已在运行，因此错过了登录。请登出到开始界面后重新登录。如果游戏刚刚更新，本应用可能也需要更新。",
        ],
    ),
    ("Synced {n} relics", ["遺物{n}個を同期しました", "유물 {n}개 동기화됨", "已同步 {n} 件遗器"]),
    (
        "Saved from your last session. Start the game and log in to bring it up to date.",
        [
            "前回のセッションで保存したものです。ゲームを起動してログインすると最新になります。",
            "지난 세션에서 저장된 내용입니다. 게임을 실행하고 로그인하면 최신 상태가 됩니다.",
            "这是上次会话保存的内容。启动游戏并登录即可更新。",
        ],
    ),
    (
        "Auto import is off, so the site keeps what it last got. Changes in game are still saved here.",
        [
            "自動インポートがオフのため、サイトは最後に受け取った内容のままです。ゲーム内の変更はここに保存され続けます。",
            "자동 불러오기가 꺼져 있어 사이트는 마지막으로 받은 내용을 유지합니다. 게임 내 변경 사항은 여기에 계속 저장됩니다.",
            "自动导入已关闭，网站会保留上次收到的内容。游戏中的变化仍会保存在这里。",
        ],
    ),
    ("Open StarRailSim to load them.", ["StarRailSimを開いて読み込みましょう。", "StarRailSim을 열어 불러오세요.", "打开 StarRailSim 即可载入。"]),
    (
        "StarRailSim is up to date. Changes you make in game show up there as they happen.",
        [
            "StarRailSimは最新です。ゲーム内の変更はその場で反映されます。",
            "StarRailSim이 최신 상태입니다. 게임에서 바꾼 내용이 바로 반영됩니다.",
            "StarRailSim 已是最新。你在游戏中的改动会即时同步过去。",
        ],
    ),
    ("Auto import", ["自動インポート", "자동 불러오기", "自动导入"]),
    (
        "Automatically imports to StarRailSim and keeps your equipment in sync.",
        [
            "StarRailSimへ自動でインポートし、装備を同期し続けます。",
            "StarRailSim으로 자동으로 불러오고 장비를 계속 동기화합니다.",
            "自动导入到 StarRailSim，并保持你的装备同步。",
        ],
    ),
    ("Launch on startup", ["スタートアップ時に起動", "시작 시 실행", "开机启动"]),
    (
        "Automatically runs on computer restart.",
        ["パソコンの再起動時に自動で起動します。", "컴퓨터를 다시 시작하면 자동으로 실행됩니다.", "电脑重启后自动运行。"],
    ),
    ("Show Nous Relic Importer", ["Nous Relic Importerを表示", "Nous Relic Importer 표시", "显示 Nous Relic Importer"]),
    ("Open StarRailSim", ["StarRailSimを開く", "StarRailSim 열기", "打开 StarRailSim"]),
    ("Settings…", ["設定…", "설정…", "设置…"]),
    ("Quit", ["終了", "종료", "退出"]),
    ("Back", ["戻る", "뒤로", "返回"]),
    // Updates.
    ("Checks for a new version shortly.", ["まもなく新しいバージョンを確認します。", "곧 새 버전을 확인합니다.", "即将检查新版本。"]),
    ("Check now", ["今すぐ確認", "지금 확인", "立即检查"]),
    ("Checking for a new version…", ["新しいバージョンを確認中…", "새 버전을 확인하는 중…", "正在检查新版本…"]),
    ("Up to date.", ["最新です。", "최신 상태입니다.", "已是最新版本。"]),
    ("Downloading version {version}…", ["バージョン{version}をダウンロード中…", "버전 {version} 다운로드 중…", "正在下载版本 {version}…"]),
    (
        "Version {version} is ready. It installs by itself once the game has been closed for a few minutes.",
        [
            "バージョン{version}の準備ができました。ゲームを閉じて数分たつと自動でインストールされます。",
            "버전 {version}이(가) 준비되었습니다. 게임을 닫고 몇 분이 지나면 자동으로 설치됩니다.",
            "版本 {version} 已就绪。关闭游戏几分钟后会自动安装。",
        ],
    ),
    ("Install now", ["今すぐインストール", "지금 설치", "立即安装"]),
    ("Version {version} is ready to install.", ["バージョン{version}をインストールできます。", "버전 {version}을(를) 설치할 수 있습니다.", "版本 {version} 可以安装了。"]),
    ("Install", ["インストール", "설치", "安装"]),
    (
        "Version {version} is ready. Installing it asks for your password, as the first install did.",
        [
            "バージョン{version}の準備ができました。最初のインストールと同じく、インストール時にパスワードを求められます。",
            "버전 {version}이(가) 준비되었습니다. 처음 설치할 때처럼 설치 시 비밀번호를 묻습니다.",
            "版本 {version} 已就绪。与首次安装一样，安装时会要求输入密码。",
        ],
    ),
    ("Version {version} is out.", ["バージョン{version}が公開されました。", "버전 {version}이(가) 나왔습니다.", "版本 {version} 已发布。"]),
    ("Download", ["ダウンロード", "다운로드", "下载"]),
    ("Installing version {version}…", ["バージョン{version}をインストール中…", "버전 {version} 설치 중…", "正在安装版本 {version}…"]),
    ("Could not update: {why}.", ["更新できませんでした：{why}。", "업데이트하지 못했습니다: {why}.", "无法更新：{why}。"]),
    ("Try again", ["再試行", "다시 시도", "重试"]),
    ("Updates are off", ["アップデートはオフです", "업데이트 꺼짐", "更新已关闭"]),
    ("Checking for updates…", ["アップデートを確認中…", "업데이트 확인 중…", "正在检查更新…"]),
    ("Install version {version}", ["バージョン{version}をインストール", "버전 {version} 설치", "安装版本 {version}"]),
    ("Download version {version}", ["バージョン{version}をダウンロード", "버전 {version} 다운로드", "下载版本 {version}"]),
    ("Check for updates", ["アップデートを確認", "업데이트 확인", "检查更新"]),
    // The window page's own words (PAGE).
    ("relics", ["遺物", "유물", "遗器"]),
    ("light cones", ["光円錐", "광추", "光锥"]),
    ("characters", ["キャラクター", "캐릭터", "角色"]),
    ("Top relic sets", ["よく使う遺物セット", "많이 쓰는 유물 세트", "常用遗器套装"]),
    (
        "Closing this window keeps sync running in the background.",
        ["このウィンドウを閉じても、同期はバックグラウンドで続きます。", "이 창을 닫아도 동기화는 백그라운드에서 계속됩니다.", "关闭此窗口后，同步仍会在后台继续。"],
    ),
    ("Settings", ["設定", "설정", "设置"]),
    ("Language", ["言語", "언어", "语言"]),
    ("Automatic", ["自動", "자동", "自动"]),
    ("Install updates automatically", ["アップデートを自動でインストール", "업데이트 자동 설치", "自动安装更新"]),
    (
        "Off: the site keeps what it last got.",
        ["オフ：サイトは最後に受け取った内容のままです。", "끄면 사이트는 마지막으로 받은 내용을 유지합니다.", "关闭时，网站保留上次收到的内容。"],
    ),
    (
        "Available once installed: run the downloaded file without --portable.",
        [
            "インストール後に使えます。ダウンロードしたファイルを --portable なしで実行してください。",
            "설치 후 사용할 수 있습니다. 다운로드한 파일을 --portable 없이 실행하세요.",
            "安装后可用：请不带 --portable 运行下载的文件。",
        ],
    ),
    ("Saved file", ["保存ファイル", "저장된 파일", "已保存的文件"]),
    ("Uninstall", ["アンインストール", "제거", "卸载"]),
    ("Last updated {ago}", ["最終更新：{ago}", "마지막 업데이트: {ago}", "最后更新：{ago}"]),
    ("Saved {ago}", ["保存：{ago}", "저장: {ago}", "保存于{ago}"]),
    ("just now", ["たった今", "방금", "刚刚"]),
    ("Last checked {ago}.", ["最終確認：{ago}。", "마지막 확인: {ago}.", "上次检查：{ago}。"]),
    (
        "Uninstall Nous Relic Importer? It stops syncing and removes itself from this computer.",
        [
            "Nous Relic Importerをアンインストールしますか？同期を停止し、このパソコンから削除されます。",
            "Nous Relic Importer를 제거할까요? 동기화를 멈추고 이 컴퓨터에서 삭제됩니다.",
            "要卸载 Nous Relic Importer 吗？它会停止同步并从这台电脑上移除。",
        ],
    ),
    ("Installing the update", ["アップデートをインストール中", "업데이트 설치 중", "正在安装更新"]),
    ("Nous Relic Importer is not running", ["Nous Relic Importerは実行されていません", "Nous Relic Importer가 실행 중이 아닙니다", "Nous Relic Importer 未在运行"]),
    (
        "The new version opens its own window in a moment, once you have entered your password.",
        [
            "パスワードを入力すると、まもなく新しいバージョンのウィンドウが開きます。",
            "비밀번호를 입력하면 잠시 후 새 버전의 창이 열립니다.",
            "输入密码后，新版本稍后会打开自己的窗口。",
        ],
    ),
    (
        "Start it from your app menu, then reload this page.",
        ["アプリメニューから起動して、このページを再読み込みしてください。", "앱 메뉴에서 실행한 뒤 이 페이지를 새로고침하세요.", "请从应用菜单启动它，然后重新加载此页面。"],
    ),
    // Messages before there is a window.
    (
        "Uninstalling Nous Relic Importer needs administrator access, to remove its sign-in task.",
        [
            "Nous Relic Importerのアンインストールには、サインイン時のタスクを削除するため管理者権限が必要です。",
            "Nous Relic Importer를 제거하려면 로그인 작업을 삭제하기 위해 관리자 권한이 필요합니다.",
            "卸载 Nous Relic Importer 需要管理员权限，以删除其登录任务。",
        ],
    ),
    (
        "Quit Nous Relic Importer from its tray icon, then uninstall it again.",
        [
            "トレイアイコンからNous Relic Importerを終了してから、もう一度アンインストールしてください。",
            "트레이 아이콘에서 Nous Relic Importer를 종료한 뒤 다시 제거하세요.",
            "请从托盘图标退出 Nous Relic Importer，然后再次卸载。",
        ],
    ),
    ("Uninstalling did not finish: {e}", ["アンインストールが完了しませんでした：{e}", "제거를 완료하지 못했습니다: {e}", "卸载未完成：{e}"]),
    ("Nous Relic Importer is uninstalled.", ["Nous Relic Importerをアンインストールしました。", "Nous Relic Importer를 제거했습니다.", "Nous Relic Importer 已卸载。"]),
    (
        "Nous Relic Importer needs administrator access to read the game's network traffic. Run it again and choose Yes.",
        [
            "Nous Relic Importerがゲームの通信を読み取るには管理者権限が必要です。もう一度実行して「はい」を選んでください。",
            "Nous Relic Importer가 게임의 네트워크 통신을 읽으려면 관리자 권한이 필요합니다. 다시 실행한 뒤 '예'를 선택하세요.",
            "Nous Relic Importer 需要管理员权限才能读取游戏的网络通信。请再次运行并选择“是”。",
        ],
    ),
    (
        "Quit Nous Relic Importer from its tray icon, then run this again to update it.",
        [
            "トレイアイコンからNous Relic Importerを終了してから、これをもう一度実行して更新してください。",
            "트레이 아이콘에서 Nous Relic Importer를 종료한 뒤 이것을 다시 실행해 업데이트하세요.",
            "请从托盘图标退出 Nous Relic Importer，然后再次运行此文件进行更新。",
        ],
    ),
    (
        "Nous Relic Importer needs your password once, to let it read the game's network traffic. Start it again and enter your password when asked. A Steam Deck has no password until you set one: open Konsole and run passwd.",
        [
            "Nous Relic Importerがゲームの通信を読み取るため、一度だけパスワードが必要です。もう一度起動し、求められたらパスワードを入力してください。Steam Deckは設定するまでパスワードがありません。Konsoleを開いて passwd を実行してください。",
            "Nous Relic Importer가 게임의 네트워크 통신을 읽으려면 비밀번호가 한 번 필요합니다. 다시 실행하고 요청되면 비밀번호를 입력하세요. Steam Deck은 설정하기 전까지 비밀번호가 없습니다. Konsole을 열고 passwd를 실행하세요.",
            "Nous Relic Importer 需要你输入一次密码，以读取游戏的网络通信。请再次启动，并在提示时输入密码。Steam Deck 在设置前没有密码：打开 Konsole 并运行 passwd。",
        ],
    ),
    (
        "This system has no password prompt for apps (pkexec), so Nous Relic Importer cannot ask for the permission it needs. Run this once in a terminal, then start it again:\n\n{command}",
        [
            "このシステムにはアプリ用のパスワード入力（pkexec）がないため、Nous Relic Importerは必要な権限を求められません。ターミナルで次を一度実行してから、もう一度起動してください：\n\n{command}",
            "이 시스템에는 앱용 비밀번호 입력(pkexec)이 없어 Nous Relic Importer가 필요한 권한을 요청할 수 없습니다. 터미널에서 다음을 한 번 실행한 뒤 다시 시작하세요:\n\n{command}",
            "此系统没有供应用使用的密码提示（pkexec），因此 Nous Relic Importer 无法请求所需权限。请在终端中运行一次以下命令，然后重新启动：\n\n{command}",
        ],
    ),
    (
        "This system did not apply the permission to read network traffic, usually because the home folder does not allow it. Run this once in a terminal, then start Nous Relic Importer again:\n\n{command}",
        [
            "このシステムでは通信を読み取る権限が適用されませんでした。多くの場合、ホームフォルダーがそれを許可していないためです。ターミナルで次を一度実行してから、Nous Relic Importerをもう一度起動してください：\n\n{command}",
            "이 시스템에서 네트워크 통신을 읽는 권한이 적용되지 않았습니다. 보통 홈 폴더가 허용하지 않기 때문입니다. 터미널에서 다음을 한 번 실행한 뒤 Nous Relic Importer를 다시 시작하세요:\n\n{command}",
            "此系统未应用读取网络通信的权限，通常是因为主文件夹不允许。请在终端中运行一次以下命令，然后重新启动 Nous Relic Importer：\n\n{command}",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::{Lang, PAGE, TABLE};

    #[test]
    fn every_translation_keeps_its_placeholders_and_the_page_has_all_its_words() {
        let names = |s: &str| {
            let mut found: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n.to_owned())).collect();
            found.sort();
            found
        };
        for (english, row) in TABLE {
            for text in row {
                assert_eq!(names(english), names(text), "{english} -> {text}");
            }
        }
        for key in PAGE {
            assert!(TABLE.iter().any(|(k, _)| k == key), "the page shows {key} with no translation");
        }
        let mut keys: Vec<&str> = TABLE.iter().map(|(k, _)| *k).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), TABLE.len(), "a key is listed twice");
    }

    #[test]
    fn locales_and_tags_map_to_our_four() {
        let cases = [("ja-JP", Some(Lang::Ja)), ("ko_KR.UTF-8", Some(Lang::Ko)), ("zh-Hans-CN", Some(Lang::Zh)), ("zh-TW", Some(Lang::Zh))];
        for (tag, lang) in cases {
            assert_eq!(Lang::from_tag(tag), lang, "{tag}");
        }
        assert_eq!(Lang::from_tag("en_US"), Some(Lang::En));
        assert_eq!(Lang::from_tag("de-DE"), None);
        assert_eq!(Lang::from_tag("C"), None);
        assert_eq!(Lang::from_tag("auto"), None);
    }
}
