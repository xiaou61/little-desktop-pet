# 桌宠角色资产

`skins/` 包含首发的三套内置资源，并通过 Rust `include_bytes!` 编译进可执行文件：

| ID | 名称 | 主资源 | 缩略图 | 来源和授权 |
| --- | --- | --- | --- | --- |
| `simple-cloud` | 简洁云朵 | `skins/simple-cloud.png` | `skins/simple-cloud-thumb.png` | Cloudling 待机首帧，来自 `rullerzhou-afk/clawd-on-desk` 提交 `bca6a8d3f275df25d620367651502c6817a0c13b` 的 `assets/gif/cloudling-idle.gif`；项目所有者确认已取得权利人书面授权，可随本应用发布。 |
| `orange-dragon` | 橙色小龙 | `skins/orange-dragon.png` | `skins/orange-dragon-thumb.png` | 本次变更中以项目内绘制的原创几何图形生成；项目自有。 |
| `calico-cat` | 三花猫 | `skins/calico-cat.png` | `skins/calico-cat-thumb.png` | 本次变更中以项目内绘制的原创几何图形生成；项目自有。 |

Cloudling 上游仓库的图稿默认条款为 `All Rights Reserved`，本项目仅依据项目所有者确认的权利人书面授权使用上述单一待机首帧。授权凭证不纳入仓库；发布该资源时必须保持上游来源、固定提交和授权依据可追溯。`references/github-skin-research/` 的其余素材仍不得打包，除非单独取得并记录可发布授权。

发布资源必须满足以下边界：

- PNG、RGBA、透明背景，四角必须完全透明。
- 主图尺寸位于 64–1024px，缩略图位于 32–256px；基准显示范围为 200 x 220 逻辑像素。
- 主体四周保留透明边距，不带矩形底色、投影、文字或外部资源。
- alpha 达到窗口模块命中阈值的像素会接收单击和拖动，其余像素会穿透到下方窗口。
- 不得把 `references/github-skin-research/` 内未获授权的参考仓库素材放入发布包；Cloudling 的已授权待机首帧除外。所有皮肤仅从本地内嵌 manifest 提供，运行时不下载资源。
