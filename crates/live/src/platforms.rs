//! 各直播平台的取流（设计文档第 6.5 节）。平台只负责“直播间 → 直播流地址、是否在播”；
//! 拉流、拆包、解码各平台共用。以后加抖音就在这里加一个平台。

use mhy_carry_core::room::LiveRoom;

use crate::bilibili::Bilibili;
use crate::client::{LiveClient, LiveError};

/// 取流结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamStatus {
    /// 在播。`urls` 是几个 CDN 上的 FLV 地址，`referer` 是拉流时要带的 Referer。
    Live {
        urls: Vec<String>,
        referer: &'static str,
    },
    Offline,
}

/// 各平台的接口地址。测试时指到本地假服务器。
pub(crate) struct Platforms {
    pub bilibili: Bilibili,
}

impl Platforms {
    pub fn official() -> Self {
        Self {
            bilibili: Bilibili::official(),
        }
    }

    pub async fn resolve(
        &self,
        client: &LiveClient,
        room: &LiveRoom,
    ) -> Result<StreamStatus, LiveError> {
        match room {
            LiveRoom::Bilibili { id } => self.bilibili.resolve(client, *id).await,
        }
    }
}
