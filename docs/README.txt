TextPilot v0.5.0
================

English
-------
TextPilot is a portable Windows AI text assistant that runs in the system tray without a main window. It requires Windows 10 or 11; Settings requires Microsoft Edge WebView2 Runtime.

1. Put TextPilot.exe in a directory writable by your user account and run it. It creates config.json beside the EXE.
2. Right-click the tray icon and choose Settings. Configure an OpenAI-compatible API endpoint, API key, models, actions, and behavior, then choose Save & Apply.
3. Select text in an application. Hold Ctrl and press F8 twice to optimize a prompt, or F9 twice to translate intelligently. Results are copied to the clipboard; paste with Ctrl+V.

TextPilot reads selections through Windows UI Automation first. If necessary, it temporarily uses and restores the clipboard. config.json stores API keys in plaintext locally; do not share it. Each action binds its own API profile and model, and can also use Standard (Double) and Deep (Triple) gestures with custom prompts. Use the tray menu to open Settings, reload externally edited configuration, or exit.

简体中文
--------
TextPilot 是一个绿色 Windows AI 文本助手，没有主窗口，运行后驻留在系统托盘。需要 Windows 10 或 Windows 11；设置窗口依赖 Microsoft Edge WebView2 Runtime。

1. 将 TextPilot.exe 放入当前用户可写目录并运行，程序会在 EXE 同目录创建 config.json。
2. 右键托盘图标，选择“设置”。配置 OpenAI 兼容的 API 地址、API Key、模型、动作和应用行为，然后点击“保存并应用”。
3. 在应用中选中文字。按住 Ctrl 连按两次 F8 优化提示词，或连按两次 F9 智能翻译。结果自动复制到剪贴板，按 Ctrl+V 粘贴。

程序优先通过 Windows UI Automation 读取选区；必要时会临时使用并恢复剪贴板。config.json 会在本地明文保存 API Key，请勿分享。每个动作独立绑定 API 配置和该配置中的模型，并支持 Double 标准模式、Triple 深度模式及自定义提示词。托盘菜单可打开设置、重新加载外部修改的配置或退出程序。
