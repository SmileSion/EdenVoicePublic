# 常见游戏识别目录

2026-09-07：扩充至 41 款游戏，保留原有 16 款，新增 25 款。服务器目录 v2。

新增进程名核对来源：[Discord 官方游戏识别目录](https://discord.com/api/v9/applications/detectable)，筛选 Windows 游戏主体，排除启动器、反作弊、模组管理器和测试程序。仅核对规则，未对全部游戏逐一实机启动验收。

| 游戏 | Windows 进程名 |
|---|---|
| Counter-Strike 2 | `cs2.exe` |
| Dota 2 | `dota2.exe` |
| 英雄联盟 | `league of legends.exe` |
| 无畏契约 | `valorant-win64-shipping.exe` |
| PUBG | `tslgame.exe` |
| Apex Legends | `r5apex.exe`, `r5apex_dx12.exe` |
| 守望先锋 | `overwatch.exe` |
| Fortnite | `fortniteclient-win64-shipping.exe` |
| 原神 | `genshinimpact.exe`, `yuanshen.exe` |
| 崩坏：星穹铁道 | `starrail.exe` |
| 艾尔登法环 | `eldenring.exe` |
| 博德之门 3 | `bg3.exe`, `bg3_dx11.exe` |
| 绝地潜兵 2 | `helldivers2.exe` |
| Minecraft 基岩版 | `minecraft.windows.exe` |
| 泰拉瑞亚 | `terraria.exe` |
| 星露谷物语 | `stardew valley.exe` |
| 三角洲行动 | `deltaforceclient-win64-shipping.exe` |
| 黎明杀机 | `deadbydaylight-win64-shipping.exe`, `deadbydaylight-egs-shipping.exe` |
| 永劫无间 | `narakabladepoint.exe` |
| 彩虹六号：围攻 | `rainbowsix.exe`, `rainbowsixgame.exe`, `rainbowsix_vulkan.exe` |
| Rust 腐蚀 | `rustclient.exe` |
| 幻兽帕鲁 | `palworld-win64-shipping.exe`, `palworld-wingdk-shipping.exe` |
| 英灵神殿 | `valheim.exe` |
| 求生之路 2 | `left4dead2.exe` |
| 深岩银河 | `fsd-win64-shipping.exe` |
| 双人成行 | `ittakestwo.exe` |
| 致命公司 | `lethal company.exe` |
| 恐鬼症 | `phasmophobia.exe` |
| 胡闹厨房 2 | `overcooked2.exe` |
| 雨中冒险 2 | `risk of rain 2.exe` |
| 森林 | `theforest.exe`, `theforestvr.exe` |
| 森林之子 | `sonsoftheforest.exe` |
| 猛兽派对 | `partyanimals.exe` |
| 盗贼之海 | `sotgame.exe`, `seaofthieves.exe` |
| Among Us | `among us.exe` |
| 星际战甲 | `warframe.exe`, `warframe.x64.exe` |
| 漫威争锋 | `marvel.exe`, `marvel-win64-shipping.exe` |
| 怪物猎人：世界 | `monsterhunterworld.exe` |
| 怪物猎人：崛起 | `monsterhunterrise.exe` |
| 严阵以待 | `readyornot-win64-shipping.exe` |
| 内容警告 | `content warning.exe` |

客户端需开启“共享正在玩的游戏”，使用支持动态目录的新版客户端。Java / .NET 通用宿主、启动器和仅靠命令行才能区分的游戏不纳入这些精确文件名规则。

上线记录：2026-09-07 19:44 CST，目录 v2 / revision 3，41 款均启用；上一版 16 款保留供管理端回退。无人在线时通过 SSH 原子写入并重启加载，程序、账号、身份、房间文件校验不变。备份目录 `/root/edenvoice-backups/20260907-194447-popular-games`。目录校验及线上目录 / 活动 / 双向加密 UDP 烟测通过。
