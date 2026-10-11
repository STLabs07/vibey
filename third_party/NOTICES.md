# Third-party assets bundled in Vibey

| Asset | Files | Source | License |
|---|---|---|---|
| VS Code default color themes (Dark/Light 2026, Modern, +, VS) | `assets/themes/*.json` | [microsoft/vscode](https://github.com/microsoft/vscode) `extensions/theme-defaults` | MIT, Copyright (c) Microsoft Corporation |
| Seti file icon theme (font + mapping) | `assets/icons/seti.ttf` (converted from `seti.woff` by `scripts/woff2ttf.py`), `assets/icons/vs-seti-icon-theme.json` | [microsoft/vscode](https://github.com/microsoft/vscode) `extensions/theme-seti`, from [jesseweed/seti-ui](https://github.com/jesseweed/seti-ui) | MIT, see `seti-ThirdPartyNotices.txt` |
| VS Code markdown grammar | `assets/grammars/markdown.tmLanguage.json` | [microsoft/vscode](https://github.com/microsoft/vscode) `extensions/markdown-basics` | MIT, Copyright (c) Microsoft Corporation |
| Codicons | `assets/icons/codicon.ttf` | [microsoft/vscode-codicons](https://github.com/microsoft/vscode-codicons) | CC BY 4.0, Copyright (c) Microsoft Corporation |
| TextMate grammars and community color themes (compiled in through the `irosashi` crate) | n/a (embedded by the dependency) | [shikijs/textmate-grammars-themes](https://github.com/shikijs/textmate-grammars-themes), the same grammars VS Code uses | Each grammar and theme keeps its own license (mostly MIT; see that repository's per-package license files). `irosashi` itself is MIT, Copyright (c) 2026 FrostyBee |

## MIT License (microsoft/vscode)

Copyright (c) 2015 - present Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Codicons: Creative Commons Attribution 4.0 International

The Codicon font is used unmodified under CC BY 4.0
(https://creativecommons.org/licenses/by/4.0/legalcode). Attribution: "Codicons" by
Microsoft Corporation, https://github.com/microsoft/vscode-codicons.
