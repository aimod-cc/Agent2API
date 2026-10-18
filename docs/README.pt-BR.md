<div align="center">

# Agent2API

**Transforme o estado de login de vários clientes de desktop de IA em um gateway de API compatível com OpenAI local**

[简体中文](../README.md) | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | **Português (BR)**

[![license](https://img.shields.io/badge/license-MIT%20%2B%20Aviso-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/Agent2API/agent2api?include_prereleases)](https://github.com/Agent2API/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/Agent2API/agent2api?style=flat&color=yellow)](https://github.com/Agent2API/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/Agent2API/agent2api/releases/latest)

[✨ Principais recursos](#-principais-recursos) • [💬 Comunidade e suporte](#-comunidade-e-suporte) • [🤝 Provedores compatíveis](#-provedores-compatíveis) • [🚀 Início rápido](#-início-rápido) • [📦 Instalação e implantação](./install.pt-BR.md) • [📸 Capturas de tela](#-capturas-de-tela) • [🔧 Desenvolvimento e build](#-desenvolvimento-e-build) • [📜 Aviso de uso](#-aviso-de-uso)

</div>

---

O Agent2API reúne as contas já conectadas de vários clientes de IA em um único endpoint compatível com OpenAI. Informe `http://127.0.0.1:3065/v1` em qualquer cliente que aceite um `base_url` personalizado para chamar a cota de modelos dessas contas diretamente — **sem API key do upstream e sem alterar o cliente**.

## ✨ Principais recursos

- 🔌 **Compatível com OpenAI** — aponte o `base_url` do seu cliente para o gateway: Chat, Responses e Messages são todos suportados
- 🏪 **17 provedores integrados** — WorkBuddy, Raccoon, CatPaw, Qoder, Trae e outros funcionam de imediato, além de provedores personalizados via passthrough
- 🔁 **Credenciais que se renovam sozinhas** — tokens são renovados antes de expirar, os próximos do vencimento podem ser atualizados em lote e o catálogo de modelos se mantém atual
- 🪂 **Rotação de várias contas** — contas entram na fila por prioridade, um 429 passa para a próxima e contas em cooldown são puladas sem espera
- 🛡️ **Proxies de saída** — cada conta pode ter seu próprio IP de saída; o fingerprint enviado imita o cliente oficial
- 🔑 **Segurança do gateway** — chaves de gateway com escopo, verificação humana no login e recusa por padrão enquanto nenhuma chave estiver configurada
- 📊 **Relatórios de requisições** — volume, taxa de sucesso e consumo de tokens num relance, com taxa de acerto de cache e mapa de calor de 365 dias
- ⏰ **Tarefas agendadas** — check-in automático, manutenção de credenciais e atualização do catálogo, tudo gerenciado numa página
- 💰 **Saldo e uso** — saldos e uso exibidos direto nas linhas das contas, com consulta concorrente entre elas
- 🖥️ **Quatro formas de rodar** — apps de desktop para Windows, macOS e Linux na bandeja (sem Node) e a imagem Docker com painel e gateway na mesma porta (amd64 / arm64); interface em seis idiomas

## ❤️ Patrocinadores

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane (patrocinador)" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>A <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> patrocina este projeto! A ProxyLane oferece proxies de IP residencial estático nativos para contas de IA (Static Residential ISP): os IPs vêm de operadoras de banda larga residenciais reais, com pontuação de fraude baixa no Scamalytics, cada IP é dedicado a você, com tráfego ilimitado, a partir de $3.90. Adicione uma saída por conta na página "Proxy de rede". Use o código <code>AGENT2API25</code> para 25% de desconto na primeira compra.</td>
</tr>

</table>

## 💬 Comunidade e suporte

- **Grupos do QQ**: grupos de chat do Agent2API — **o grupo 1 (`1127656296`) está quase lotado, prefira o grupo 2 (`1130894674`)**; pesquise o número no QQ para entrar.
- **Grupo do Telegram**: <https://t.me/+9gM8YPvgRL02NGRl>
- **Servidor do Discord**: <https://discord.gg/z9GzSJMWp>
- **💖 Doação**: se o Agent2API ajuda você, fique à vontade para [pagar um café ao autor](./SPONSOR.md); doações são voluntárias e não desbloqueiam nada.
- **🤝 Patrocínio / negócios**: quer patrocinar o projeto ou divulgar seu produto aqui? Envie um e-mail para [541675139@qq.com](mailto:541675139@qq.com).

## 🤝 Provedores compatíveis

Panorama dos recursos de proxy reverso por plataforma (✓ suportado · ✗ não suportado · — não se aplica):

| Plataforma | Requisições LLM | Renovação automática do token | Lista de modelos | Consulta de saldo | Check-in | Resgates |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy (China) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| WorkBuddy (global) | ✓ | ✓ | ✓ | ✓ | ✗ | — |
| 小浣熊 | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| CatPaw | ✓ | ✗ | ✓ | ✓ | ✗ | — |
| AutoClaw (China / global) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Qoder | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Cline (Free / Pass) | ✓ | ✓ | ✓ | ✓ | — | — |
| Accio (global / China) | ✓ | ✓ | ✓ | ✓ | — | — |
| ZCode (China / global) | ✓ | ✗ | ✗ | ✓ | — | ✓ |
| CodeArts | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| Trae | ✓ | ✓ | ✓ | ✓ | — | — |
| Loomy (iFlytek) | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| KukuAI (Baidu Wenku) | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| MonkeyCode (长亭科技, China / global) | ✓ | ✗ | ✓ | — | — | — |
| Command Code | ✓ | ✗ | ✓ | — | — | — |
| Antigravity (Google, Gemini / Claude / GPT-OSS) | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce (Huawei Cloud, OfficeClaw) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| OrcaTerm (Tencent Cloud) | ✓ | ✓ | ✓ | ✓ | — | — |
| Zhipu Qingyan | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Provedores personalizados | ✓ | — | ✓ | — | — | — |

> Nota: em Lista de modelos, ✓ significa busca automática online (algumas plataformas também mantêm uma tabela estática interna como fallback); o ZCode tem apenas a tabela estática interna. ✗ em Renovação automática do token significa que a plataforma não tem um fluxo de renovação disponível (a conta continua funcionando enquanto as credenciais forem válidas). O check-in do Qoder é somente na versão da China.
>
> O **Zhipu Qingyan** é o único provedor aqui que suporta um **modo visitante**: sem login — um clique obtém uma credencial que já funciona para conversar (a cota gratuita é emitida pela origem por dispositivo). A contrapartida é que uma conta de visitante não tem sistema de pontos — as colunas "Saldo" / "Check-in" / "Resgate" acima, além das **tarefas de novato e diárias do centro de tarefas**, valem apenas para **contas conectadas** (cole um token ou importe o estado de login do cliente desktop); visitantes são ignorados de forma transparente.

`/v1/chat/completions`, `/v1/responses`, `/v1/messages` (incl. `count_tokens`) e `/v1/models` funcionam igual para todas as plataformas; mapeamento de modelos, fila global de prioridade, fallback em 429, proxy de saída, mascaramento de impressão digital de saída e relatórios de requisições valem para todas as plataformas.

> **Este projeto é apenas para aprendizado e troca de conhecimento.** Ele reutiliza o estado de login das suas contas na forma de um cliente não oficial, o que pode violar os termos dos serviços upstream; os riscos (inclusive banimento da conta) são seus. Uso comercial e contorno de cobrança são proibidos. Veja o [Aviso de uso](#-aviso-de-uso) e a [LICENSE](../LICENSE).
>
> Este é um projeto pessoal de ferramenta de proxy local, sem vínculo com qualquer fornecedor upstream ou seus produtos oficiais (a lista está no [Aviso de uso](#-aviso-de-uso)); os formatos de interface vêm da observação do tráfego dos clientes, e o upstream pode mudar a qualquer momento.

## 🚀 Início rápido

Três formas — escolha a que combina com o seu ambiente; **o guia completo de instalação, atualização, desinstalação e solução de problemas está em [Instalação e implantação](./install.pt-BR.md)**.

| Forma | Artefato | Interface |
| --- | --- | --- |
| **Desktop** (Windows / macOS / Linux) | `Agent2API_<versão>_x64-setup.exe`, `_universal.dmg`, `_amd64.deb`, `_amd64.AppImage` | Janela do app |
| **Servidor** (Windows Server / Linux, sem GUI) | `agent2api-server-windows-amd64.zip`, `agent2api-server-linux-<arch>.tar.gz` | Painel no navegador |
| **Docker** | `aimodcc/agent2api:latest` (amd64 / arm64) | Painel no navegador |

**Desktop**: instale e abra → adicione uma conta na página "Contas" → aponte o `base_url` do cliente para `http://127.0.0.1:3065/v1` (o `api_key` pode ser qualquer coisa, por exemplo `sk-local`). Não é preciso Node nem outro runtime: o gateway roda dentro do processo do aplicativo e fica na bandeja.

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

### Central de check-in

![Central de check-in: visão geral e grupos de check-in diário por provedor](../assets/screenshots/checkin-center.png)

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

### Apenas para aprendizado e discussão

Este projeto é um exercício prático de proxy reverso HTTP, passthrough de streaming SSE, adaptação de protocolos de múltiplos upstreams e empacotamento desktop (Tauri), e destina-se **apenas a aprendizado e pesquisa pessoal**. Não é um produto oficial e não tem qualquer vínculo, endosso ou patrocínio de Tencent e WorkBuddy / CodeBuddy, Meituan e CatPaw, SenseTime e 小浣熊, Zhipu e AutoClaw / autoglm, Alibaba e Qoder / Accio, Huawei Cloud e CodeArts / OfficeAce, ByteDance e Trae, iFlytek e Loomy, Baidu Wenku e KukuAI, 长亭科技 e MonkeyCode, Command Code, ou Google e Antigravity.

### Sobre o comportamento de proxy reverso

O que este projeto implementa é um proxy reverso local: ele reutiliza o estado de login das suas próprias contas, na sua própria máquina, e encaminha as requisições para os gateways oficiais dos upstreams. Ele não quebra nem contorna qualquer verificação de pagamento ou permissão — a cota utilizada vem sempre do que a sua própria conta já possui. Dito isso, é preciso deixar claro que essa forma de encaminhar "reutilizando o estado de login na forma de um cliente não oficial" **pode não estar de acordo com os termos de uso dos serviços upstream**; usar ou não, e toda consequência disso (incluindo, mas não se limitando a, limitação de taxa, sinalização por controle de risco, congelamento ou banimento da conta), é responsabilidade do usuário.

### Usos proibidos

É proibido usar este projeto para qualquer finalidade comercial, redistribuição com fins lucrativos, operação de contas em massa, contorno de cobrança ou limites de cota do upstream, ou qualquer atividade que viole as leis e regulamentos locais. Para chamar esses serviços de modelos em produção ou em cenários comerciais, use os canais oficiais e as APIs oficiais.

### Credenciais e risco de dados

Este projeto armazena as credenciais das contas (`accessToken` / `refreshToken` etc.) em **texto puro** no diretório de configuração local (por padrão `~/.agent2api/`), e os arquivos gerados pela exportação também contêm credenciais em texto puro. Guarde-os com cuidado: nunca faça commit em repositório público, não envie para armazenamento em nuvem nem compartilhe com terceiros. Prejuízos causados por vazamento de credenciais são de responsabilidade do usuário.

### Sem garantias e aviso de direitos

Este projeto é fornecido "como está"; o autor não faz qualquer promessa sobre disponibilidade, estabilidade, segurança ou adequação a um propósito específico. As interfaces do upstream podem mudar a qualquer momento, e o projeto pode parar de funcionar e ficar sem manutenção a qualquer momento. Os termos completos estão na [LICENSE](../LICENSE). As formas de interface, campos de protocolo e outras informações deste projeto vêm da observação e organização do tráfego de rede publicamente visível dos clientes oficiais; as marcas e serviços relacionados pertencem aos seus respectivos donos. Se um detentor de direitos considerar este projeto inadequado, entre em contato com o autor, e ele será ajustado ou removido prontamente.

## 📄 Licença

Este projeto é distribuído sob a [MIT License](../LICENSE); você pode usá-lo, modificá-lo e distribuí-lo livremente desde que mantenha o aviso de copyright.

Uma ressalva: a LICENSE traz um **Aviso de uso** após o texto da MIT, cujo item 3 **adiciona restrições** à MIT (proibição de uso comercial, proibição de redistribuição com fins lucrativos, proibição de operação de contas em massa). Portanto este projeto **não** é MIT puro — **os termos da MIT e o Aviso de uso juntos formam a licença completa**, e onde os dois chegam a conclusões diferentes sobre o mesmo ato, prevalece o mais restritivo. É também por isso que o `Cargo.toml` aponta `license-file` para a LICENSE em vez de declarar o identificador SPDX `"MIT"`.

## ⭐ Star History

<a href="https://star-history.com/#Agent2API/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[Baixe a versão mais recente](https://github.com/Agent2API/agent2api/releases/latest)** • **[Reporte um problema](https://github.com/Agent2API/agent2api/issues)**

Se este projeto for útil para você, uma ⭐️ Star é muito bem-vinda!

</div>
