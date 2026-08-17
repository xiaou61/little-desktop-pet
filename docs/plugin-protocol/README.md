# `.petpack` 插件协议

`.petpack` 是一个只包含声明式资源的 ZIP 文件。宿主只读取根目录的
`manifest.json` 和 manifest 引用的资源，不执行 JavaScript、Rust DLL、WASM、脚本或其他进程。

## Manifest

```json
{
  "manifestVersion": 1,
  "id": "example-skin",
  "version": "1.0.0",
  "displayName": "示例皮肤",
  "kind": "skin",
  "hostApi": "1",
  "permissions": [],
  "contributes": [
    {
      "id": "example-skin.skin",
      "type": "skins",
      "resource": "skins/pet.png",
      "thumbnail": "skins/thumb.png",
      "width": 200,
      "height": 220
    }
  ]
}
```

贡献 ID 必须使用插件 ID 命名空间。首版支持 `skins`、`panelCards`、`settings`
和 `menus`；权限只能使用宿主协议声明的白名单。皮肤资源必须是带透明边界的
RGBA PNG。路径必须是包内相对路径，不能包含 `..`、绝对路径或脚本扩展名。

资源包上限由宿主固定：压缩包 64 MiB、最多 256 个文件、展开总量 32 MiB、
单文件 8 MiB，主图最大 2048 px，缩略图最大 512 px。超过任一限制时，整个包
都会被拒绝，不会留下半安装目录。

## Bun SDK 边界

未来的 Bun/TypeScript SDK 只负责生成 manifest、检查资源和打包 `.petpack`。
它不是桌宠运行时依赖，也不能授予网络、任意文件、窗口句柄、SQLite 或动态
Tauri command 权限。插件运行时只接受上述声明式协议。

`examples/petpack/demo-skin.petpack` 是一个可离线导入的最小示例包；它使用项目
内置的透明云朵资源，仅用于协议和安装流程测试。
