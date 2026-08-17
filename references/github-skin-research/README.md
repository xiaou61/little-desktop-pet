# 桌宠皮肤参考预览

这是本次皮肤探索用的临时参考资料，不会自动接入“小桌宠”应用。`previews/` 中的文件来自下列 GitHub 仓库的固定提交，先用于比较视觉方向；其中 Cloudling 的待机首帧已经在项目所有者确认取得权利人书面授权后，作为 `simple-cloud` 皮肤接入。其余素材仍只作参考，除非另行确认可发布授权。`previews/` 含有第三方素材，因此仅保留在本地研究目录，不提交到项目 Git 仓库。

其中 `clawd-idle-preview.png`、`calico-idle-preview.png` 和 `cloudling-idle-preview.png` 是从对应 GIF 提取的首帧预览；前两者只做了展示用的透明/背景处理，原始 GIF 保持不变。

## 候选方向

| 编号 | 方向 | 代表预览 | 适合观察的点 | 许可证与备注 |
| --- | --- | --- | --- | --- |
| A | 像素生物 / 多宠物包 | `previews/openpets-default.png`、`previews/openpets-cat-balloon.webp`、`previews/openpets-dragon.webp` | 角色轮廓、帧动画、不同物种共用皮肤接口 | OpenPets 仓库为 MIT；具体资源发布前仍需逐项确认资产声明 |
| B | 同骨架多主题 | `previews/clawd-idle.gif`、`previews/calico-idle.gif`、`previews/cloudling-idle.gif` | 同一套状态动画替换不同角色，最接近“皮肤切换”产品形态 | `clawd-on-desk` 为 AGPL-3.0，图稿另有保留权利条款；仅 `cloudling-idle.gif` 的待机首帧已获权利人授权接入，其余仍只作参考 |
| C | 猫 / 企鹅陪伴组合 | `previews/mitarashi.webp`、`previews/mitarashi-penguin.png`、`previews/mitarashi-running-cat.webp` | 可爱动物、静态待机与奔跑姿态、多个宠物同时存在 | Desktop Pet Mitarashi 为 MIT；复用前仍保留版权与许可证 |
| D | 经典 Neko 像素猫 | `previews/oneko.gif` | 极简小体积、方向行走、低资源动画 | 仓库提供 MIT 风格许可文本；复用时保留版权声明 |

## 来源

- A：[`alvinunreal/openpets`](https://github.com/alvinunreal/openpets)，提交 `32abea1dd05f3f8e905d4ae9b9b9be08444dd3a8`
- B：[`rullerzhou-afk/clawd-on-desk`](https://github.com/rullerzhou-afk/clawd-on-desk)，提交 `bca6a8d3f275df25d620367651502c6817a0c13b`
- C：[`Sunwood-ai-labs/desktop-pet-mitarashi`](https://github.com/Sunwood-ai-labs/desktop-pet-mitarashi)，提交 `5f8d4d75fe397e9bc4e26af542651eb170809ae0`
- D：[`adryd325/oneko.js`](https://github.com/adryd325/oneko.js)，提交 `5281d057c4ea9bd4f6f997ee96ba30491aed16c0`

## 选择建议

- 想要“一个角色换很多外观”：优先看 **B**，它最能验证主题/皮肤数据结构。
- 想要“可爱但不局限于人形”：优先看 **C**，猫和企鹅的体型差异比较明显。
- 想要“占比小、资源轻、桌面感强”：优先看 **D**。
- 想要“未来扩展成宠物商店或插件包”：优先看 **A**。

当前不建议直接复用带有明确第三方角色商标或角色身份的素材；你选定的是视觉方向后，我们可以制作同样气质的原创皮肤，再进入 OpenSpec 规范阶段。
