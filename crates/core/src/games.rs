//! 支持的游戏。加一个游戏就是在 `GAMES` 里加一条记录。

/// 一个游戏（一个服务器）的静态配置。
#[derive(Debug, PartialEq, Eq)]
pub struct GameProfile {
    /// 界面上显示的游戏名。确认弹窗里的游戏名只用这里的，不用二维码里的 `app_name`。
    pub display_name: &'static str,
    /// 二维码和扫码接口里的 `app_id`。
    pub app_id: u32,
    /// 二维码里的 `biz_key`，同时也是 panda 扫码接口路径里的那一段，比如 `hk4e_cn`。
    pub biz_key: &'static str,
    /// passport 扫码（scanQRLogin）返回的应用 ID。确认登录前要核对它，
    /// 防止把账号登录到别的应用上。
    pub passport_app_id: &'static str,
    /// 用来自动找游戏窗口的进程名。
    pub process_names: &'static [&'static str],
}

pub static GAMES: &[GameProfile] = &[GameProfile {
    display_name: "原神",
    app_id: 4,
    biz_key: "hk4e_cn",
    // 最初来自其他开源项目的抓包记录，M1 已用真实账号确认（设计文档第 14 节）。
    passport_app_id: "c76ync6mutq8",
    process_names: &["YuanShen.exe"],
}];

/// 按二维码里的 `app_id` 和 `biz_key` 找游戏，两者必须属于同一条记录。
pub fn find(app_id: u32, biz_key: &str) -> Option<&'static GameProfile> {
    GAMES
        .iter()
        .find(|game| game.app_id == app_id && game.biz_key == biz_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_genshin_by_app_id_and_biz_key() {
        let game = find(4, "hk4e_cn").expect("应该找到原神");
        assert_eq!(game.display_name, "原神");
        assert_eq!(game.passport_app_id, "c76ync6mutq8");
        assert_eq!(game.process_names, &["YuanShen.exe"]);
    }

    #[test]
    fn app_id_and_biz_key_must_match_the_same_game() {
        assert!(find(4, "nap_cn").is_none());
        assert!(find(12, "hk4e_cn").is_none());
        assert!(find(4, "HK4E_CN").is_none());
    }
}
