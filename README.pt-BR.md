# Agent2API · Gateway local multi-provedor

[简体中文](./README.md) | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | **Português (BR)**

Empacota o estado de login de vários clientes de desktop de IA em um **gateway de API compatível com OpenAI** local, expondo um único `base_url` e trazendo junto gerenciamento de contas multi-provedor, gerenciamento de modelos (ativar / excluir / mapear), mascaramento de impressão digital de saída, proxy de saída e relatórios de requisições — além de um aplicativo desktop Tauri pronto para usar. Qualquer cliente OpenAI que aceite um `base_url` personalizado pode chamar a cota de modelos desses provedores por `http://127.0.0.1:3065/v1` — sem API key e sem alterar o código do cliente.

Panorama dos recursos de proxy reverso por plataforma (✓ suportado · ✗ não suportado · — não se aplica):

| Plataforma | Requisições LLM | Renovação automática do token | Lista de modelos (atualização remota) | Consulta de saldo | Check-in | Resgates |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy (China) | ✓ | ✓ | ✓ remota + fallback estático | ✓ | ✓ check-in diário | — |
| WorkBuddy (global) | ✓ | ✓ | ✓ remota + fallback estático | ✓ | ✗ sem evento de check-in | — |
| 小浣熊 | ✓ | ✓ | ✓ remota + fallback estático | ✓ | ✓ pontos por login no desktop | — |
| CatPaw | ✓ | ✗ sem mecanismo de renovação | ✓ remota + fallback estático | ✓ | ✗ | — |
| AutoClaw (China / global) | ✓ | ✓ | ✓ remota + fallback estático | ✓ | ✓ check-in diário | — |
| Qoder | ✓ | ✓ | ✓ remota (por região) + fallback estático | ✓ | ✓ somente China | — |
| Cline (Free / Pass) | ✓ | ✓ | ✓ remota + fallback estático | ✓ | — | — |
| Accio (global / China) | ✓ | ✓ | ✓ remota + fallback estático | ✓ apenas percentual de uso | — | — |
| ZCode (China / global) | ✓ | ✗ | ✗ tabela estática | ✓ saldo do plano | — | ✓ plano por tempo limitado (manual) |
| CodeArts | ✓ | ✓ rotação única | ✓ remota (três fontes mescladas) | ✓ dois balanços | — | ✓ bônus diário (manual) |
| Trae | ✓ | ✓ troca a cada uso | ✓ somente remota | ✓ dois balanços | — | — |
| Loomy (iFlytek) | ✓ | ✗ sem API de renovação | ✓ somente remota | ✓ dois balanços de pontos | ✓ atualização dos pontos diários | — |
| KukuAI (Baidu Wenku) | ✓ | ✗ sem API de renovação | ✓ remota + fallback estático | ✓ saldo de pontos | ✓ check-in diário (pontos grátis) | — |
| MonkeyCode (长亭科技, China / global) | ✓ | ✗ sem API de renovação | ✓ somente remota | — | — | — |
| Command Code | ✓ | ✗ API key estática | ✓ remota + fallback estático | — | — | — |
| Antigravity (Google, Gemini) | ✓ | ✓ | ✓ remota + fallback estático | — | — | — |
| OfficeAce (Huawei Cloud, OfficeClaw) | ✓ | ✓ renovação automática do plano de controle | ✓ somente remota | ✓ saldo de pontos | ✓ bônus diário (check-in + presente de boas-vindas) | — |
| Provedores personalizados | ✓ passthrough de chat / Responses / Anthropic | — | ✓ registro manual + busca no servidor | — | — | — |

Os três pontos de entrada de conversa (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`, além de `/v1/messages/count_tokens`) e `/v1/models` funcionam igual para todas as plataformas; as diferenças acima dizem respeito apenas ao que cada upstream consegue fazer. Mapeamento de modelos, fila global de prioridade, fallback em 429, proxy de saída, mascaramento de impressão digital e relatórios de requisições valem para todas as plataformas.

> **Este projeto é apenas para aprendizado e troca de conhecimento.** Ele reutiliza o estado de login das suas contas na forma de um cliente não oficial, o que pode violar os termos dos serviços upstream; os riscos (inclusive banimento da conta) são seus. Uso comercial e contorno de cobrança são proibidos. Veja o [Aviso de uso](#aviso-de-uso) e a [LICENSE](./LICENSE).
>
> Este é um projeto pessoal de ferramenta de proxy local, sem vínculo com qualquer fornecedor upstream ou seus produtos oficiais (a lista está no [Aviso de uso](#aviso-de-uso)); os formatos de interface vêm da observação do tráfego dos clientes, e o upstream pode mudar a qualquer momento.

---

## Índice

- [Início rápido](#início-rápido)
- [Implantação com Docker](#implantação-com-docker)
- [Capturas de tela](#capturas-de-tela)
- [Estrutura do projeto](#estrutura-do-projeto)
- [Desenvolvimento e build](#desenvolvimento-e-build)
- [Aviso de uso](#aviso-de-uso)
- [Licença](#licença)

---

## Início rápido

Baixe o instalador na página de Releases (NSIS, chinês simplificado, instala por padrão em `C:\Program Files\Agent2API` e exige aprovação de administrador durante a instalação) e basta iniciá-lo — **não é necessário instalar Node nem qualquer outro runtime**.

1. Na primeira execução, o gateway local (porta 3065) sobe dentro do processo do aplicativo e a janela principal abre. Se um diretório ou arquivo de dados de uma versão antiga for encontrado, um diálogo orienta a migração — basta seguir as instruções.
2. Clique em "Adicionar conta" na página de Contas, escolha um provedor e siga o diálogo: entre ou preencha as credenciais (login pela web / código por SMS / colar credenciais / importar o estado de login desta máquina).
3. Aponte o `base_url` do seu cliente OpenAI para `http://127.0.0.1:3065/v1` e preencha o `api_key` com qualquer coisa (por exemplo `sk-local`; o servidor não valida enquanto a autenticação estiver desativada).

Fechar a janela apenas minimiza para a bandeja, e o gateway continua encaminhando em segundo plano; para sair de verdade, clique com o botão direito no ícone da bandeja e escolha "Sair".

### Verificação

Com o gateway no ar, use curl para confirmar a conectividade (coloque em `model` qualquer nome que exista de fato em `GET /v1/models`):

```bash
curl http://127.0.0.1:3065/health
curl http://127.0.0.1:3065/v1/models

curl http://127.0.0.1:3065/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"你好"}],"stream":true}'
```

Exemplo de integração de cliente (SDK de Python):

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:3065/v1", api_key="sk-local")
resp = client.chat.completions.create(
    model="deepseek-v4.1-flash",     # o provedor que tiver o modelo — veja GET /v1/models
    messages=[{"role": "user", "content": "你好"}],
)
print(resp.choices[0].message.content)
```

**Páginas no navegador** (uma interface web própria, …) que chamam este endpoint com `fetch` esbarram no preflight cross-origin — o gateway **não** responde CORS por padrão, então o preflight cai na verificação de API key e recebe 401. Duas saídas: ① ative "Segurança → Acesso cross-origin do gateway" nas Configurações (origem `*`; com `*` qualquer página pode acionar o seu gateway, então configure também uma "Gateway Key"); ② coloque a página na mesma origem — sirva-a de um pequeno servidor estático local que faz proxy reverso de `/v1` para `127.0.0.1:3065`.

### Acesso pela rede local

Por padrão o gateway escuta apenas `127.0.0.1`. Ao ativar "Configurações → Geral → Acesso pela rede local" ele passa a escutar todos os adaptadores e os dispositivos da rede só precisam apontar o endereço da API para o IP desta máquina (a interface mostra o endereço completo). Por segurança, é obrigatório registrar um administrador do painel antes de ativar: a API de gerenciamento passa a exigir sessão de administrador ou uma Gateway Key e, sem chave ativa, a API de encaminhamento também recusa (o fluxo cria uma chave "padrão" automaticamente). O painel web pode ser exposto à rede local opcionalmente (fechado por padrão). As mudanças valem após reiniciar.

---

## Implantação com Docker

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

Abra `http://<host>:3065` no navegador: a primeira visita guia o **cadastro do administrador** (usado nos logins seguintes); depois de entrar, crie uma API key na página "Gateway Keys" para os clientes — `http://<host>:3065/v1` é o endpoint compatível com OpenAI; sem nenhuma chave ele recusa o encaminhamento e, após criar a primeira, volta a funcionar sozinho. Todo o estado (banco SQLite / configuração / logs) fica em um único volume `./data`.

Para quem usa compose (há imagens amd64 e arm64):

```yaml
services:
  agent2api:
    image: aimodcc/agent2api:latest
    container_name: agent2api
    restart: unless-stopped
    ports:
      - "3065:3065"
    volumes:
      - ./data:/data
```

Variáveis de ambiente (todas opcionais — nada precisa ser predefinido):

| Variável | Descrição |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | Predefine o administrador e a senha (a senha vai em texto puro e é transformada em hash na inicialização). Sem isso, use o cadastro no painel |
| `AGENT2API_PANEL_PORT` | Painel em porta separada: ao definir, o painel (interface + `/api/*`) escuta só nessa porta e basta mapear a porta principal publicamente para manter o plano de gerenciamento interno (vincule a porta do painel ao loopback, como `127.0.0.1:3066:3066`) |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | Endereço de escuta (padrão `0.0.0.0`) / porta (padrão `3065`) |
| `AGENT2API_ALLOW_NO_KEY` | `1` desativa o fail-closed (serve `/v1` mesmo sem chave configurada — só para redes internas) |
| `AGENT2API_CAPTCHA_ENABLED` | Widget de verificação humana na tela de login: `1` ativado (padrão), `0` desativado |

Compilar a partir do código: clone o repositório e rode `docker compose up -d --build` (a imagem contém apenas o gateway e o painel, sem toolchain Rust).

**Diferenças de recursos no painel web** (todas decorrem de "não haver cliente de desktop local"): login pela web (WorkBuddy / Qoder / Cline), código por SMS e credenciais coladas funcionam plenamente; logins pela web que exigem callback nesta máquina (AutoClaw / CatPaw / Accio / CodeArts / Trae) e a "importação do estado de login local" não estão disponíveis — use credenciais coladas.

---

## Capturas de tela

### Contas

Todas as contas ficam em uma fila única (a segunda coluna é a prioridade) e podem ser ativadas individualmente; cooldown por modelo, validade e saldo aparecem na linha, e saldo baixo é pulado abaixo do limiar por padrão.

![Página de contas: fila global, cooldown de limite por modelo, validade e saldo](./assets/screenshots/accounts.png)

Escolha o provedor e entre; o mesmo fornecedor pode manter várias versões de conta (ex.: WorkBuddy China / global) e o encaminhamento escolhe pelo nome do modelo:

![Adicionar conta: escolha o provedor e a versão, depois faça login pela web](./assets/screenshots/add-account.png)

### Relatório

Visão geral: requisições / taxa de sucesso / tokens / modelo mais usado, com rankings por conta e provedor, roscas de uso e mapa de calor de 365 dias:

![Visão geral do relatório: cartões de estatística, top contas / provedores, roscas de uso por modelo e por provedor](./assets/screenshots/report-overview.png)

Tendências: **taxa de acerto de cache** (linha) e **consumo de tokens** (área) em dois eixos, com barras de tokens por dia abaixo:

![Tendências do relatório: taxa de acerto de cache e consumo de tokens em dois eixos, barras de tokens por dia](./assets/screenshots/report-trends.png)

### Tarefas agendadas

As tarefas de segundo plano ficam todas nesta página (ativar / intervalo / último resultado / executar agora); a lista fica em `scheduledTasks` no `~/.agent2api/config.json` e as alterações valem na hora.

![Página de tarefas agendadas: ativação e intervalos de check-in, manutenção de credenciais, atualização do catálogo de modelos e mais](./assets/screenshots/scheduled-tasks.png)

---

## Estrutura do projeto

O gateway e o aplicativo desktop ficam ambos em `desktop-tauri/`: o backend é um servidor HTTP Rust em processo dentro de `src-tauri/`, e o frontend é HTML/CSS/JS puro em `ui/`.

```
agent2api/
├─ desktop-tauri/
│  ├─ src-tauri/
│  │  ├─ server/                 Crate do gateway (agent2api-server, compilado de forma independente:
│  │  │                          compartilhado pelo app desktop e pelo binário headless; a implementação
│  │  │                          em src/server/ e o bin/agent2api-server.rs não têm dependência de GUI)
│  │  │  ├─ mod.rs               Montagem do serviço: ServerState, inicialização, parada, migração de inicialização
│  │  │  ├─ http.rs              Tabela de rotas, CORS, middleware de API Key, limite de body, hospedagem estática headless
│  │  │  ├─ config.rs / logging.rs / logs_store.rs / errors.rs
│  │  │  ├─ config_migration.rs  Migração do diretório de configuração 1.x (~/.workbuddy-proxy → ~/.agent2api, primeiro passo na inicialização)
│  │  │  ├─ request_stats.rs + request_stats/   Janelas de tempo, gravação, agregação e poda das estatísticas
│  │  │  ├─ core/
│  │  │  │  ├─ providers/        ★ Camada multi-provedor (o coração desta reformulação)
│  │  │  │  │  ├─ mod.rs        ProviderKind (fornecedores internos; Cline dividido em dois pools de cota, Accio e
│  │  │  │  │  │                ZCode cada um em duas regiões) + registro PROVIDERS + consultas de id
│  │  │  │  │  ├─ adapter.rs    Trait ProviderAdapter + adapter_for + implemented_kinds
│  │  │  │  │  ├─ router.rs     Nome do modelo → conjunto de provedores candidatos (catálogo agregado)
│  │  │  │  │  ├─ catalog.rs    Catálogo de modelos agregado (mesclagem de listas / deduplicação por nome / disponibilidade)
│  │  │  │  │  ├─ catalog_cache.rs  Cache persistente da lista remota de cada provedor (lida de volta ao reiniciar, sem cair na lista interna)
│  │  │  │  │  ├─ refresh_flight.rs  Deduplicação single-flight da renovação de credenciais
│  │  │  │  │  ├─ workbuddy.rs  Adaptador WorkBuddy (conjunto de cabeçalhos / injeção de system / 6004 / 11-128)
│  │  │  │  │  ├─ raccoon/      小浣熊: mod / models / credentials / jwt / oauth / balance
│  │  │  │  │  ├─ catpaw/       CatPaw: adapter (is_stateful) / conversation (máquina de estados de turno) /
│  │  │  │  │  │                turn_executor / prepare / decision (decisão de turno) / fingerprint /
│  │  │  │  │  │                registry/ (registro de sessões: tabela e handles / identidade da conta / invalidação) /
│  │  │  │  │  │                messages / blocks / tools / openai (camada de tradução) /
│  │  │  │  │  │                upstream_http / image_compress / models / credentials / balance
│  │  │  │  │  ├─ autoclaw/     Zhipu autoglm (China + global, dois provedores): region (domínios e identidade por região) /
│  │  │  │  │  │                adapter / credentials / refresh / crypto / models /
│  │  │  │  │  │                balance / login (código por SMS, só China) /
│  │  │  │  │  │                oauth (login pela web Zai / Google, só global) / checkin (tarefa de check-in diário)
│  │  │  │  │  ├─ qoder/        Qoder: adapter / endpoints (endereços dos dois sites) / oauth (autorização por dispositivo) /
│  │  │  │  │  │                auth / cosy (assinatura COSY e codificação do body) / protocol (decodificação do envelope) /
│  │  │  │  │  │                chat (encaminhamento em estilo sessão) / stream / machine (PKCE e identificação da máquina) /
│  │  │  │  │  │                credentials / refresh / models / balance
│  │  │  │  │  ├─ cline/        Cline: adapter (Bearer + cabeçalhos de produto) / credentials (prefixo workos:
│  │  │  │  │  │                + estado de login do desktop + parsing do nome) / login (autorização de dispositivo WorkOS) /
│  │  │  │  │  │                refresh (renovação single-flight) / models (dois pools de cota + sementes de mapeamento padrão) /
│  │  │  │  │  │                balance (saldo de credit, micro credit ÷1e6)
│  │  │  │  │  ├─ accio/        Accio (global + China, dois provedores): endpoints (duas regiões e caminhos) /
│  │  │  │  │  │                credentials / auth / refresh (renovação single-flight) /
│  │  │  │  │  │                oauth (login pela web PKCE + callback loopback) /
│  │  │  │  │  │                models (fallback estático + /api/llm/config/v2) /
│  │  │  │  │  │                protocol (envelope estilo Gemini do OpenAI ↔ ADK) /
│  │  │  │  │  │                chat (encaminhamento em estilo sessão) / stream (desempacote do SSE do ADK) / balance
│  │  │  │  │  ├─ zcode/        ZCode (Zhipu Z.AI, China + global): region (dois domínios e identidades) /
│  │  │  │  │  │                adapter (stateless, parametrizado por região) / credentials (token + JWT do plano) /
│  │  │  │  │  │                oauth (login por polling da CLI) / coding_key (troca por API key de inferência) / models (tabela estática) /
│  │  │  │  │  │                balance (saldo do plano) / plan + claim (canal do plano e resgate por tempo limitado) /
│  │  │  │  │  │                captcha (pool de tokens de verificação humana) / reasoning (orçamento de pensamento GLM-5.3) /
│  │  │  │  │  │                zcode_system.json (prompt de sistema)
│  │  │  │  │  ├─ codearts/     CodeArts (Huawei Cloud): signer (SDK-HMAC-SHA256 da Huawei Cloud,
│  │  │  │  │  │                conferido byte a byte com a implementação de referência) / credentials / dpop (proof DPoP ES256) /
│  │  │  │  │  │                oauth (login pela web PKCE + callback loopback) / refresh (renovação single-flight) /
│  │  │  │  │  │                session (heartbeat de chat-session e admissão simultânea por conta) / chat (encaminhamento em estilo sessão) /
│  │  │  │  │  │                stream_fault (envelopes de erro dentro de um HTTP 200) / redact (mascaramento do corpo de erro) /
│  │  │  │  │  │                models (mesclagem das três fontes agent / builtin / gateway de benefícios) /
│  │  │  │  │  │                balance (estatísticas de assinatura + dois balanços do gateway de benefícios) /
│  │  │  │  │  │                welfare (resgate diário: chave de idempotência persistida antes, releitura para confirmação dupla)
│  │  │  │  │  ├─ trae/         Trae (IDE de IA da ByteDance, canal SOLO): credentials / device (par de chaves do dispositivo) /
│  │  │  │  │  │                login + oauth (login pela web PKCE + candidatos de troca de token) / callback_server
│  │  │  │  │  │                (callback em porta aleatória local com filtragem de ruído) / refresh (renovação single-flight) /
│  │  │  │  │  │                payload (reconstrução do envelope SOLO por lista branca) / headers (conjunto de cabeçalhos SOLO) /
│  │  │  │  │  │                stream (tradução SSE→chunk) / forward (encaminhamento com estado) /
│  │  │  │  │  │                errors (classificação de erros e lista de configurações mortas) / models (catálogo get_detail_param) /
│  │  │  │  │  │                usage (pacotes de benefício + cota do plano, dois balanços) / profile (parsing de identidade)
│  │  │  │  │  ├─ loomy/        Loomy (iFlytek): login (código por SMS) / credentials (session de 14 dias, sem API de renovação) /
│  │  │  │  │  │                sign (reproduz os cabeçalhos de assinatura HMAC-SHA1 do cliente) / endpoints / client (gateway de integração) /
│  │  │  │  │  │                models (catálogo remoto /api/v1/models, sem lista de fallback embutida no upstream) /
│  │  │  │  │  │                balance (pontos permanentes + pontos diários, dois balanços) / checkin (o primeiro login do dia atualiza os pontos dados)
│  │  │  │  │  ├─ kuku/         KukuAI (Baidu Wenku "Kuku AI / GenFlowPro", kuku.baidu.com):
│  │  │  │  │  │                adapter (is_stateful) / session (cache do trio bdstoken/uinfo/uk) /
│  │  │  │  │  │                engine (troca de STOKEN) / http / login (login pela web no site + captura de cookie no shell) /
│  │  │  │  │  │                credentials (cookies BDUSS) / models (fallback estático + atualização remota) /
│  │  │  │  │  │                chat (criar sessão → alocar computação → SSE) / balance (pontos) /
│  │  │  │  │  │                checkin (pontos grátis diários)
│  │  │  │  │  ├─ monkeycode/   MonkeyCode (长亭科技, China + global, dois provedores): region (domínios e identidade dos dois sites) /
│  │  │  │  │  │                adapter / endpoints (caminhos / nome do cookie / mapa interface_type→CLI) /
│  │  │  │  │  │                client / credentials (session + imageId) / login (colar session e descoberta automática) /
│  │  │  │  │  │                models (uma lista por site, somente remota) / task (criar tarefa) / stream (stream de tarefas via WS) /
│  │  │  │  │  │                translate (eventos ACP → quadros de chat, com aprovação automática de ferramentas e resposta automática a perguntas)
│  │  │  │  │  ├─ commandcode/  Command Code: adapter / endpoints / credentials / login /
│  │  │  │  │  │                fingerprint (impressão digital determinística do dispositivo + relatório a cada 8h) / models /
│  │  │  │  │  │                plan (envelope de 8 chaves e reescrita de params, derivação do session id)
│  │  │  │  │  ├─ antigravity/  Antigravity (Google, Gemini): oauth (renovação por refresh token + single-flight) /
│  │  │  │  │                   project (descoberta loadCodeAssist → onboardUser) / credentials /
│  │  │  │  │                   endpoints (bases dos três ambientes e conjunto de cabeçalhos) / login /
│  │  │  │  │                   models (fetchAvailableModels + fallback interno + mapeamento do nome real no upstream) / adapter
│  │  │  │  │  └─ officeace/    OfficeAce (Huawei Cloud, OfficeClaw): oauth (login self-service PKCE + P-256 DPoP) /
│  │  │  │  │                   signer (assinaturas SDK-HMAC e V11-HMAC + fixtures de vetores) /
│  │  │  │  │                   credentials (Basic do gateway + AK/SK temporárias do plano de controle) /
│  │  │  │  │                   chat (OpenAI compatível sem estado) / models (catálogo remoto + multiplicadores por modelo) /
│  │  │  │  │                   probe (sondagem de disponibilidade; modelos inacessíveis ficam ocultos por padrão) /
│  │  │  │  │                   subscription (assinatura / cota / recompensas) / onboarding / checkin / balance
│  │  │  │  ├─ protocol/        Camada de conversão de protocolo (wire de cada fornecedor ↔ chat SSE padrão:
│  │  │  │  │                    Anthropic Messages / Responses / NDJSON (Command Code) /
│  │  │  │  │                    envelope Gemini (Antigravity), além de planos de ferramentas e reparo de histórico)
│  │  │  │  ├─ upstream/        Orquestração do encaminhamento: laço da fila global de contas (provider_loop) +
│  │  │  │  │                    tratamento do corpo enviado (payload) + passthrough/agregação de SSE + extração de usage
│  │  │  │  │                    em via lateral + ligação dos fluxos de tradução para protocolos não chat (translate: Anthropic / NDJSON / Gemini)
│  │  │  │  ├─ account_store/   Armazenamento de contas (prioridade global, cooldown de limite, inclusão e importação por fornecedor)
│  │  │  │  ├─ models/          Base do catálogo de modelos (lista interna do workbuddy + atualização /v3/config)
│  │  │  │  ├─ model_rules.rs   Regras de gerenciamento de modelos (desativar / ocultar / alias de mapeamento)
│  │  │  │  ├─ api_keys.rs      Lista de chaves do gateway (várias chaves; qualquer uma ativa já passa)
│  │  │  │  ├─ auth.rs / auth_http.rs / login.rs   Sessões, transporte de saída, login headless
│  │  │  │  │                    (login/ guarda os fluxos específicos: o callback loopback
│  │  │  │  │                    do catpaw, a autorização por dispositivo do qoder)
│  │  │  │  ├─ routing.rs / billing/   Roteamento de contas (prioridade global + cooldown de limite) / operação de check-in de pontos
│  │  │  │  ├─ proxies.rs / clash.rs / egress.rs   Proxies de saída e Client com cache por saída
│  │  │  │  ├─ sanitize.rs      Mascaramento de impressão digital de saída (conjunto de regras fixas: remoção de cabeçalhos + reescrita mínima de frases-modelo)
│  │  │  │  ├─ prompt.rs        Prompt de sistema próprio do gateway (passthrough / substituir / anexar)
│  │  │  │  ├─ degrade.rs       Máquina de estados de degradação por bloqueio de conteúdo (prompt neutro até as 00:00 do dia seguinte)
│  │  │  │  ├─ credential_maintenance.rs  Renovação em lote de credenciais vencidas / perto de vencer
│  │  │  │  ├─ usage_query.rs     Consultas de saldo / pontos (concorrência entre contas + snapshot da rodada agendada)
│  │  │  │  ├─ scheduled_tasks.rs  Registro e laço de despacho das tarefas agendadas por intervalo (ativar / intervalo /
│  │  │  │  │                      último resultado; configuração em scheduledTasks no config.json)
│  │  │  │  └─ account_transfer.rs + account_transfer/ / auto_checkin.rs / update/
│  │  │  │                       Importação/exportação (com normalização de identidade) / check-in agendado / atualização do software
│  │  │  └─ api/                 Handlers de cada rota (health/session/accounts/accounts_usage/
│  │  │                          chat/models/keys/model_manage/stats/logs/billing/
│  │  │                          sanitize/prompt/auto-checkin/scheduled-tasks/update/…)
│  │  ├─ lib.rs                 Entrada do app (migração do diretório de config → configurações → bandeja → janela principal → start do backend)
│  │  ├─ backend.rs              Ciclo de vida do servidor em processo
│  │  ├─ legacy_install.rs       Limpeza da instalação antiga "usuário atual" (diretório / atalhos / entrada de desinstalação / inicialização automática; só release)
│  │  ├─ gateway.rs              Cliente HTTP do shell para a API de gerenciamento
│  │  ├─ login.rs / commands.rs  Janela de login e polling, comandos invoke expostos ao frontend
│  │  ├─ login_profile.rs        Cada login pela web ganha um diretório temporário de WebView2 exclusivo (apagado ao terminar)
│  │  ├─ bridge.rs               Script de ponte injetado como window.workbuddyDesktop
│  │  └─ update.rs / settings.rs / state.rs / tray.rs
│  ├─ ui/                        Frontend (HTML/CSS/JS puro, sem framework)
│  └─ src-tauri/tauri.conf.json  Configuração de empacotamento (NSIS)
├─ build/make-icon.mjs           Gera a imagem-fonte do ícone do aplicativo
├─ assets/screenshots/           Imagens usadas nos READMEs (capturas da interface)
├─ Dockerfile / .dockerignore    Imagem headless (build multi-stage, só gateway e painel)
├─ docker-compose.yml / .env.example   Implantação (contêiner único: painel + gateway na mesma porta)
└─ package.json                  Entradas dos scripts de build (tauri:dev / tauri:build / build:icon)
```

---

## Desenvolvimento e build

### Requisitos

- Rust >= 1.77 e o toolchain do Tauri 2 (para compilar o app desktop; no Windows também é preciso o runtime WebView2)
- Node.js >= 18.17 (apenas para rodar `npm run tauri:*` e scripts de build do frontend, como `build/make-icon.mjs`; o app desktop não depende de Node em tempo de execução e não empacota nenhum artefato de Node)

### Scripts comuns

```bash
npm run tauri:install      # Instala as dependências do desktop (equivalente a npm --prefix desktop-tauri install)
npm run tauri:dev          # Abre o app desktop em modo de desenvolvimento (com hot reload)
npm run tauri:build        # Compila o instalador do desktop

npm run build:icon         # Gera a imagem-fonte do ícone (rode após mudar o design do ícone e depois rode tauri icon)
```

O projeto raiz não tem dependências de runtime; o `package.json` apenas fornece os atalhos de script acima. O artefato de build é `target/release/bundle/nsis/Agent2API_<versão>_x64-setup.exe` (hoje com cerca de 3,0 MB; o `src-tauri/.cargo/config.toml` aponta o `target-dir` do cargo para o `target/` na raiz do projeto).

---

## Aviso de uso

### Apenas para aprendizado e discussão

Este projeto é um exercício prático de proxy reverso HTTP, passthrough de streaming SSE, adaptação de protocolos de múltiplos upstreams e empacotamento desktop (Tauri), e destina-se **apenas a aprendizado e pesquisa pessoal**. Não é um produto oficial e não tem qualquer vínculo, endosso ou patrocínio de Tencent e WorkBuddy / CodeBuddy, Meituan e CatPaw, SenseTime e 小浣熊, Zhipu e AutoClaw / autoglm, Alibaba e Qoder / Accio, Huawei Cloud e CodeArts / OfficeAce, ByteDance e Trae, iFlytek e Loomy, Baidu Wenku e KukuAI, 长亭科技 e MonkeyCode, Command Code, ou Google e Antigravity.

### Sobre o comportamento de proxy reverso

O que este projeto implementa é um proxy reverso local: ele reutiliza o estado de login das suas próprias contas, na sua própria máquina, e encaminha as requisições para os gateways oficiais dos upstreams. Ele não quebra nem contorna qualquer verificação de pagamento ou permissão — a cota utilizada vem sempre do que a sua própria conta já possui. Dito isso, é preciso deixar claro que essa forma de encaminhar "reutilizando o estado de login na forma de um cliente não oficial" **pode não estar de acordo com os termos de uso dos serviços upstream**; usar ou não, e toda consequência disso (incluindo, mas não se limitando a, limitação de taxa, sinalização por controle de risco, congelamento ou banimento da conta), é responsabilidade do usuário.

### Usos proibidos

É proibido usar este projeto para qualquer finalidade comercial, redistribuição com fins lucrativos, operação de contas em massa, contorno de cobrança ou limites de cota do upstream, ou qualquer atividade que viole as leis e regulamentos locais. Para chamar esses serviços de modelos em produção ou em cenários comerciais, use os canais oficiais e as APIs oficiais.

### Credenciais e risco de dados

Este projeto armazena as credenciais das contas (`accessToken` / `refreshToken` etc.) em **texto puro** no diretório de configuração local (por padrão `~/.agent2api/`), e os arquivos gerados pela exportação também contêm credenciais em texto puro. Guarde-os com cuidado: nunca faça commit em repositório público, não envie para armazenamento em nuvem nem compartilhe com terceiros. Prejuízos causados por vazamento de credenciais são de responsabilidade do usuário.

### Sem garantias e aviso de direitos

Este projeto é fornecido "como está"; o autor não faz qualquer promessa sobre disponibilidade, estabilidade, segurança ou adequação a um propósito específico. As interfaces do upstream podem mudar a qualquer momento, e o projeto pode parar de funcionar e ficar sem manutenção a qualquer momento. Os termos completos estão na [LICENSE](./LICENSE). As formas de interface, campos de protocolo e outras informações deste projeto vêm da observação e organização do tráfego de rede publicamente visível dos clientes oficiais; as marcas e serviços relacionados pertencem aos seus respectivos donos. Se um detentor de direitos considerar este projeto inadequado, entre em contato com o autor, e ele será ajustado ou removido prontamente.

---

## Licença

Este projeto é distribuído sob a [MIT License](./LICENSE); você pode usá-lo, modificá-lo e distribuí-lo livremente desde que mantenha o aviso de copyright.

Uma ressalva: a LICENSE traz um **Aviso de uso** após o texto da MIT, cujo item 3 **adiciona restrições** à MIT (proibição de uso comercial, proibição de redistribuição com fins lucrativos, proibição de operação de contas em massa). Portanto este projeto **não** é MIT puro — **os termos da MIT e o Aviso de uso juntos formam a licença completa**, e onde os dois chegam a conclusões diferentes sobre o mesmo ato, prevalece o mais restritivo. É também por isso que o `Cargo.toml` aponta `license-file` para a LICENSE em vez de declarar o identificador SPDX `"MIT"`.

---

## Star History

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>
