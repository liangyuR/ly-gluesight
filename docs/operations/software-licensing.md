# 授权部署与现场操作

## 为安装包配置授权

1. 在 Grantmint 创建 GlueSight 应用并取得应用 UUID、签名 key ID、公钥；不要复制服务端私钥。
2. 复制 `packaging/license.example.json` 到仓库外受控路径，填写真实 `serverUrl`（HTTPS）、`productId`、`trustedKeys`。不要在文件中添加明文密码字段。
3. 在 Windows PowerShell 运行：

   ```powershell
   ./scripts/New-LicenseProvisioning.ps1 -Config <应用公钥配置.json> -OutputDirectory <尚不存在的私有目录>
   ```

   工具先限制目录权限，再生成固定随机密码及 Argon2id 校验值。`license.local.json` 用于构建；`emergency-password.txt` 仅供维护人员保管，不提交、不同客户共享、不随安装包交付。工具拒绝覆盖已有文件，后续构建复用原配置即可保持固定密码。

4. 本机正式构建前设置 `GLUESIGHT_LICENSE_CONFIG` 为配置的绝对路径，再使用原有 `pnpm package` 入口。这个环境变量仅在构建阶段读取，不是运行时开关。缺少或无效的正式配置会阻止构建；普通开发构建缺配置时可看页面但不能开始受限业务。
5. GitHub 发包使用仓库 Actions secret `GLUESIGHT_LICENSE_CONFIG_JSON` 保存 `license.local.json` 内容；CI只将它写到 runner 临时目录后编译，不上传配置文件。PR测试无需生产配置。此任务不会自动为客户创建生产应用或设置真实签名密钥。

公钥轮换时在配置中保留旧key ID，加入新公钥后重新构建。产品UUID变更不是普通升级，会与已存设备身份冲突；不要通过更换产品ID“修复”现场问题。

## 客户端激活与续约

从侧栏进入“软件授权”，填写管理员交付的授权 UUID 和激活秘密。成功后核对本机停用时间与设备摘要。只在首次激活和主动更新证书时联网。

续约后点击“更新授权”。后台已重签、客户端尚未领取时，仍执行本机旧到期时间。网络失败不会自动给予宽限期，不应删除安全存储后重试。设备确认失败与证书保存失败应分开处理：以本机已验证期限为准，重新更新可补发确认。

七天提醒在运行中的客户端显示；软件关闭期间不发送邮件、短信或系统服务通知。到期后授权页和已有记录查看/导出保持可用。

## 72 小时现场应急

维护人员在授权页输入固定密码、填写现场原因，点击“临时解锁 72 小时”，核对持续提示中的截止时刻。不要把密码发给客户作为长期使用办法；本机事件记录在受保护授权锚中。

应急不会向服务端修改合同期限，到期后必须已有有效正式授权或重新由维护人员处理。错误密码受节流限制，应急窗口不能重复点击叠加期限。正常生产恢复前仍须核对PLC、相机及配方就绪状态，不自动启动已停止的业务。

DPAPI依赖原Windows账号。删除设备身份/可信锚、换账号或迁移整机不能用重建文件规避；应保留原账号资料备份，由维护人员核对设备和授权。备份不要混入项目导出包。全机快照回滚、程序被管理员篡改不属于纯离线软件可完全防护的范围。

## 验证入口

```powershell
pnpm exec vitest run tests/licensing.test.tsx
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib licensing::
cargo test --locked --manifest-path tools/license-provision/Cargo.toml
node --test --test-name-pattern='release build refuses missing licensing' tests/packaging/windows-workflow.test.mjs
```

PLC和工件准入专项用例见[测试说明](../testing/TESTING.md)。真实服务联调只能对隔离授权、测试密钥和临时安全存储进行，不能用生产客户授权反复激活。

真实 Grantmint 专项默认标记 ignored，需要显式运行。每次创建新的测试授权，准备仓库外受限 JSON 文件，包含 `config`（上面的客户端配置）、`licenseId`、`activationSecret`、`admin`（测试后台的 `username` 和 `password`）。把路径传给仅测试使用的 `GLUESIGHT_LICENSE_E2E_CONFIG`，执行：

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib grantmint_live_activation_secure_restart_sync_and_acknowledgement -- --ignored
```

该测试实际激活一台新设备，重开临时 DPAPI 存储，调用测试后台续期到第二版，检查新期限及服务端确认，再用不可达回环地址验证离线准入与更新失败保留证书。测试会修改该测试授权；测试管理员口令、激活秘密和生成的密码文件均不得提交或上传为报告。
