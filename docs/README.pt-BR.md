<div align="center">

# Agent2API

**Gateway de roteamento de IA + gerenciamento de clientes Agent, unificando APIs de modelos e configurações locais**

[English](../README.md) | [简体中文](./README.zh-CN.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | **Português (BR)**

[![license](https://img.shields.io/badge/license-AGPL--3.0-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/Agent2API?include_prereleases)](https://github.com/aimod-cc/Agent2API/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/Agent2API?style=flat&color=yellow)](https://github.com/aimod-cc/Agent2API/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/aimod-cc/Agent2API/releases/latest)

[✨ Principais recursos](#-principais-recursos) • [💬 Comunidade e suporte](#-comunidade-e-suporte) • [🚀 Início rápido](#-início-rápido) • [📦 Instalação e implantação](./install.pt-BR.md) • [📸 Capturas de tela](#-capturas-de-tela) • [🔧 Desenvolvimento e build](#-desenvolvimento-e-build) • [📜 Aviso de uso](#-aviso-de-uso)

</div>

---

O Agent2API é um **gateway de roteamento de IA com gerenciamento de clientes Agent**. Ele conecta provedores personalizados a uma API unificada e centraliza o roteamento de contas, mapeamento de modelos, chaves do gateway e relatórios. Também permite importar, alternar e visualizar previamente as configurações de conexão de clientes Agent locais.

## ✨ Principais recursos

- 🔌 **APIs de modelos unificadas** — OpenAI Chat / Responses / Images, Anthropic Messages e catálogo unificado; Raccoon, Loomy, Antigravity, ModelScope, WorkBuddy e Doubao Work geram imagens, e Raccoon, Loomy, Antigravity, ModelScope e WorkBuddy também editam imagens de referência. Ambos os recursos estão disponíveis como ferramentas do assistente de IA
- 🧭 **Roteamento de IA** — modelos, prioridade de contas, balanceamento, afinidade de sessão e failover
- 🤖 **Gerenciamento de clientes Agent** — vários perfis, importação de configurações, prévia de gravação e alternância de serviços
- 🧩 **Extensões por plugins** — provedores adicionais instalados e atualizados separadamente; provedores personalizados dispensam plugins
- 🛡️ **Proxies de saída** — saídas do pool de proxies para contas e tarefas de atualização
- 🔑 **Segurança do gateway** — chaves com escopo, verificação humana no painel e recusa de modelos sem chave configurada
- 📊 **Relatórios** — volume, sucesso, tokens, acerto de cache e mapa de calor de 365 dias
- ⏰ **Manutenção em segundo plano** — modelos, credenciais e atualizações; o check-in diário roda no horário de janela de cada provedor (Qoder 10:01, ZCode 19:01, os demais seguem o horário da agenda), com uma rodada de recuperação que só trata contas ainda não confirmadas no dia; tarefas de provedores adicionais vêm de plugins
- 🖥️ **Três formas de execução** — desktop Windows / macOS / Linux, servidor Windows / Linux e Docker (amd64 / arm64); sem runtime Node, interface em seis idiomas

## ❤️ Patrocinadores

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane (patrocinador)" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>A <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> patrocina este projeto! A ProxyLane oferece proxies de IP residencial estático nativos para contas de IA (Static Residential ISP): os IPs vêm de operadoras de banda larga residenciais reais, com pontuação de fraude baixa no Scamalytics, cada IP é dedicado a você, com tráfego ilimitado, a partir de $3.90. Adicione uma saída por conta na página "Proxy de rede". Use o código <code>AGENT2API25</code> para 25% de desconto na primeira compra.</td>
</tr>

</table>

## 💬 Comunidade e suporte

- **Grupos do QQ**: grupos de chat do Agent2API — **os grupos 1 (`1127656296`) e 2 (`1130894674`) estão quase lotados, entre no grupo 3 (`1108276647`)**; pesquise o número no QQ para entrar.
- **Grupo do Telegram**: <https://t.me/+9gM8YPvgRL02NGRl>
- **💖 Doação**: se o Agent2API ajuda você, fique à vontade para [pagar um café ao autor](./SPONSOR.md); doações são voluntárias e não desbloqueiam nada.
- **🤝 Patrocínio / negócios**: quer patrocinar o projeto ou divulgar seu produto aqui? Envie um e-mail para [541675139@qq.com](mailto:541675139@qq.com).

## 🚀 Início rápido

Três formas — escolha a que combina com o seu ambiente; **o guia completo de instalação, atualização, desinstalação e solução de problemas está em [Instalação e implantação](./install.pt-BR.md)**.

| Forma | Artefato | Interface |
| --- | --- | --- |
| **Desktop** (Windows / macOS / Linux) | `Agent2API_<versão>_x64-setup.exe`, `_universal.dmg`, `_amd64.deb`, `_amd64.AppImage` | Janela do app |
| **Servidor** (Windows Server / Linux, sem GUI) | `agent2api-server-windows-amd64.zip`, `agent2api-server-linux-<arch>.tar.gz` | Painel no navegador |
| **Docker** | `aimodcc/agent2api:latest` (amd64 / arm64) | Painel no navegador |

**Desktop**: adicione um provedor e uma conta, crie uma chave do gateway e aplique o perfil na página "Agent", ou configure o cliente com `http://127.0.0.1:3065/v1` e essa chave. Recursos adicionais exigem o plugin correspondente.

**Servidor** (sem GUI; registrado como serviço do sistema e iniciado no boot):

```bash
# Linux
tar -xzf agent2api-server-linux-amd64.tar.gz && cd agent2api-server-linux-amd64 && sudo bash install.sh
```

```powershell
# Windows Server (PowerShell como administrador)
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg; .\install-service.ps1
```

**Docker**:

```bash
docker run -d --name agent2api --restart unless-stopped -p 3065:3065 -v ./data:/data aimodcc/agent2api:latest
```

Em qualquer forma, abra o painel depois (`http://127.0.0.1:3065/`; use o IP do host nas formas servidor e Docker), registre o administrador e crie uma chave na página "Gateway Keys". Teste de conectividade: `curl http://127.0.0.1:3065/health`

**Celular**: abra `http://<IP do gateway>:3065/m` no navegador (dá para "Adicionar à tela inicial" e virar um PWA em tela cheia). Para um ícone próprio, instale o `Agent2API-<versão>.apk` (Android) ou `Agent2API-<versão>.ipa` (iOS, não assinado — reassine você mesmo) do Release. Detalhes: [Instalação e implantação → Celular](install.pt-BR.md#celular).

## 📸 Capturas de tela

### Contas

![Página de contas: fila global, cooldown de limite por modelo, validade e saldo](../assets/screenshots/accounts.png)

Adicionar conta:

![Adicionar conta: escolha o provedor e a versão, depois faça login pela web](../assets/screenshots/add-account.png)

### Gerenciamento de modelos

![Gerenciamento de modelos: catálogo por provedor com taxa, contexto e mapeamentos](../assets/screenshots/models.png)

### Relatório

Visão geral: requisições / taxa de sucesso / tokens / modelo mais usado, com rankings por conta e provedor, roscas de uso e mapa de calor de 365 dias:

![Visão geral do relatório: cartões de estatística, top contas / provedores, roscas de uso por modelo e por provedor](../assets/screenshots/report-overview.png)

Tendências: **taxa de acerto de cache** (linha) e **consumo de tokens** (área) em dois eixos, com barras de tokens por dia abaixo:

![Tendências do relatório: taxa de acerto de cache e consumo de tokens em dois eixos, barras de tokens por dia](../assets/screenshots/report-trends.png)

## 🔧 Desenvolvimento e build

### Requisitos

- Rust >= 1.77 e o toolchain do Tauri 2 (para compilar o app desktop; no Windows também é preciso o runtime WebView2)
- Node.js >= 18.17 (apenas para rodar `npm run tauri:*` e scripts de build do frontend; o app desktop não depende de Node em tempo de execução)
- Plataformas alvo: desktops Windows / macOS / Linux (bundles por plataforma: NSIS no Windows, app+dmg no macOS, deb+AppImage no Linux) e servidor Docker

### Scripts comuns

```bash
npm run tauri:install      # Instala as dependências do desktop (equivalente a npm --prefix desktop-tauri install)
npm run tauri:dev          # Abre o app desktop em modo de desenvolvimento (com hot reload)
npm run tauri:build        # Compila o instalador do desktop

npm run build:icon         # Gera a imagem-fonte do ícone (rode após mudar o design do ícone e depois rode tauri icon)
```

O artefato de build é `target/release/bundle/nsis/Agent2API_<versão>_x64-setup.exe` (cerca de 8,6 MB na 3.0; o `src-tauri/.cargo/config.toml` aponta o `target-dir` do cargo para o `target/` na raiz do projeto).

O `.dmg` do macOS **só pode ser compilado no macOS** (`npm --prefix desktop-tauri run build -- --target universal-apple-darwin`) — não há compilação cruzada a partir do Windows, então o artefato oficial sai do workflow `build` da CI (`macos-latest`, dois alvos mesclados). Os pacotes do desktop Linux saem do job `linux-desktop` do mesmo workflow (deb + AppImage); as imagens Docker e os pacotes de servidor usados na atualização dentro do contêiner saem do job `linux-server`, que reaproveita o caminho de compilação do Dockerfile, de modo que o executável dentro do pacote vem da mesma toolchain da imagem.

## 📜 Aviso de uso

### Segurança de credenciais e dados

As credenciais são criptografadas no banco local, mas exportações, backups e arquivos de chave podem conter dados sensíveis. Não os envie para repositórios públicos, nuvem ou terceiros.

### Sem garantias

O projeto é fornecido "como está", sem garantia de disponibilidade, estabilidade, segurança ou adequação. Marcas e serviços pertencem aos respectivos titulares. Consulte a [LICENSE](../LICENSE).

## 📄 Licença

O projeto usa a mesma [GNU Affero General Public License v3.0 (AGPL-3.0)](../LICENSE) do repositório de plugins. Uso, modificação e distribuição, inclusive comerciais, são permitidos conforme a licença. Preserve avisos de copyright e licença e cumpra as obrigações aplicáveis de código-fonte. Ao oferecer uma versão modificada como serviço de rede, forneça aos usuários o código-fonte correspondente.

## ⭐ Star History

<a href="https://star-history.com/#aimod-cc/Agent2API&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
  </picture>
</a>

<div align="center">

**[Baixe a versão mais recente](https://github.com/aimod-cc/Agent2API/releases/latest)** • **[Reporte um problema](https://github.com/aimod-cc/Agent2API/issues)**

Se este projeto for útil para você, uma ⭐️ Star é muito bem-vinda!

</div>
