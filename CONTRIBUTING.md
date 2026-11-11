# 开发流程 / Development workflow

`dev` 是日常集成分支，`main` 用于稳定版本。功能与修复使用独立分支，验证后合并进 `dev`。

1. 从最新的 `dev` 创建 `feat/<topic>` 或 `fix/<topic>`。
2. 在功能分支提交相关代码与测试，保留其他尚未提交的工作。
3. 运行 `cargo test`、`cargo clippy --all-targets -- -D warnings` 和相关平台的编译检查；修改原生输入时，记录 Windows → macOS 实机验证结果或待验证项目。
4. 审查改动后通过 PR 或本地 `git merge --no-ff <feature-branch>` 合并进 `dev`，保留功能分支的合并记录。
5. 稳定版本再由 `dev` 合入 `main`。推送、发布及安装包分发按当前任务的授权执行。

Use `dev` for integration and `main` for stable releases. Start `feat/<topic>` or `fix/<topic>` from the current `dev`, commit the related code and tests, validate, and merge into `dev` through a PR or a local `--no-ff` merge. Preserve unrelated uncommitted work. For native input changes, record real Windows → macOS results or list the pending hardware checks. Promote `dev` to `main` for a stable release; push and publish according to the scope authorized for the task.

macOS 本机构建使用固定的 `Luxru Development` 自签名证书。运行 `npx @tauri-apps/cli@2 build --bundles app` 保留证书签名，不使用 `--no-sign` 或 `codesign -s -`，以免更换应用身份导致旧授权失效。其他开发者可用 `APPLE_SIGNING_IDENTITY` 指定自己的证书。
