# EdenVoice 品牌图标

2026-09-09：按已确认的双叶、圆环和五段声波方案制作的矢量母版。

- `logo-light.svg` / `logo-dark.svg`：透明底独立标志。
- `wordmark-light.svg` / `wordmark-dark.svg`：横向文字组合（使用系统字体）。
- `app-icon-light.svg` / `app-icon-dark.svg`：圆角底应用图标。
- `tray-light.svg` / `tray-dark.svg`：明亮背景用深色、深色背景用白色，无细叶脉。
- `preview.html`：日夜对照，以及实际尺寸预览。

## 生成

先在 desktop 安装依赖，然后从仓库根目录运行：

```sh
node desktop/scripts/generate-brand.mjs
```

统一几何路径、颜色与生成规则在此脚本中维护。脚本重新生成 SVG、Tauri PNG / ICO / ICNS 和 Windows Store 尺寸，并同步产品页与 Web 管理后台的图标。

## 使用

客户端内图标随显式 light / dark 或系统主题切换。安装包、桌面快捷方式和 Dock 使用统一的深色应用图标；系统安装图标不随客户端主题切换。

macOS 托盘使用黑色透明模板，由 AppKit 自动处理菜单栏颜色。Windows 读取 `SystemUsesLightTheme`，在启动、主题事件和窗口重新聚焦时更新；系统主题与客户端所选主题互不干扰。Linux 面板没有统一的主题检测接口，使用彩色底图标以确保对比度。

图标资源更新本身不改变版本号，也不会自动部署或重新发布安装包。开发客户端重启后加载新的原生托盘资源；已安装的客户端需重新打包更新后才会变化。
