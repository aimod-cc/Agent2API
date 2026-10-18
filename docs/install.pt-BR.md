# Instalação e implantação

Esta página é o **guia completo de instalação e operação** das três formas de entrega: **desktop** (Windows / macOS / Linux), **servidor** (Windows / Linux, sem interface gráfica) e **Docker**. Se não souber qual escolher, comece pela tabela abaixo.

Depois de instalar, qualquer forma é gerenciada pelo **painel no navegador** (contas, chaves de gateway, relatórios) — o painel usa a mesma porta do gateway; a forma desktop tem também a janela do aplicativo.

[简体中文](./install.md) | [English](./install.en.md) | [繁體中文](./install.zh-Hant.md) | [日本語](./install.ja.md) | [한국어](./install.ko.md) | **Português (BR)**

## Sumário

- [Qual forma escolher](#qual-forma-escolher)
- [Desktop (Windows / macOS / Linux)](#desktop-windows--macos--linux)
- [Servidor (Windows / Linux, sem interface)](#servidor-windows--linux-sem-interface)
- [Docker](#docker)
- [Verificação e conexão do cliente](#verificação-e-conexão-do-cliente)
- [Acesso pela rede local](#acesso-pela-rede-local)
- [Celular](#celular)

## Qual forma escolher

Escolha a linha que corresponde à sua máquina:

| Forma | Artefato | Interface | Observações |
| --- | --- | --- | --- |
| **Desktop Windows** (recomendado) | `Agent2API_<versão>_x64-setup.exe` | Janela do app | Instalador NSIS, instala em `C:\Program Files\Agent2API` por padrão e exige aprovação de administrador |
| **Desktop macOS** | `Agent2API_<versão>_universal.dmg` | Janela do app | Binário universal (Apple Silicon e Intel compilados separadamente e mesclados); macOS 10.15+ |
| **Desktop Linux** | `Agent2API_<versão>_amd64.deb` ou `Agent2API_<versão>_amd64.AppImage` | Janela do app | O deb instala no sistema (requer webkit2gtk-4.1 / gtk3); o AppImage é um único arquivo autossuficiente que roda em qualquer distribuição |
| **Servidor Windows** (sem GUI) | `agent2api-server-windows-amd64.zip` | **Painel no navegador** | Instalação por linha de comando, registrado como serviço do Windows — veja [Servidor](#servidor-windows--linux-sem-interface) |
| **Servidor Linux** (sem GUI) | `agent2api-server-linux-<arch>.tar.gz` | **Painel no navegador** | Instalação por linha de comando, sob systemd; o mesmo pacote também é o payload da atualização dentro do contêiner |
| **Docker** | `aimodcc/agent2api:latest` | **Painel no navegador** | Imagens amd64 / arm64, veja [Implantação com Docker](#docker) |

> Qual escolher: **você tem um desktop** (quer bandeja, iniciar com o sistema, importar o estado de login local) → app de desktop;
> **o servidor não tem interface gráfica** (Windows Server / servidor Linux) → o pacote de servidor;
> **já usa Docker** → a imagem.
>
> A forma desktop exige uma sessão gráfica e **não pode ser instalada em servidor sem interface**.
> O pacote de servidor não linka WebView / GTK: um executável e um diretório `ui/` bastam.

## Desktop (Windows / macOS / Linux)

Os **apps de desktop** (Windows / macOS / Linux) bastam ser iniciados após a instalação — **não é necessário instalar Node nem qualquer outro runtime**; o gateway roda dentro do processo do aplicativo e permanece na bandeja.

1. Na primeira execução, o gateway local (porta 3065) sobe dentro do processo do aplicativo e a janela principal abre. Se um diretório ou arquivo de dados de uma versão antiga for encontrado, um diálogo orienta a migração — basta seguir as instruções.
2. Clique em "Adicionar conta" na página de Contas, escolha um provedor e siga o diálogo: entre ou preencha as credenciais (login pela web / código por SMS / colar credenciais / importar o estado de login desta máquina).
3. Aponte o `base_url` do seu cliente OpenAI para `http://127.0.0.1:3065/v1` e preencha o `api_key` com qualquer coisa (por exemplo `sk-local`; o servidor não valida enquanto a autenticação estiver desativada).

Fechar a janela apenas minimiza para a bandeja, e o gateway continua encaminhando em segundo plano; para sair de verdade, clique com o botão direito no ícone da bandeja e escolha "Sair".

> **Primeira abertura no macOS**: o app dentro do dmg não tem assinatura de código nem notarização da Apple (assinar exige uma conta paga de Apple Developer — veja a explicação em `.github/workflows/build.yml`), então o Gatekeeper bloqueia a primeira execução. Libere uma vez em "Ajustes do Sistema → Privacidade e Segurança" e ele abre normalmente depois.
>
> **Notas do desktop Linux**: a bandeja depende de `libayatana-appindicator3` (a maioria dos ambientes traz; instale `libayatana-appindicator3-1` e reabra se o ícone não aparecer). Um AppImage precisa do bit de execução antes da primeira execução (`chmod +x`; o app o define automaticamente ao escolher "Abrir instalador"). Atualizações: o deb passa pelo instalador do sistema (ele pede sua senha uma vez), enquanto o AppImage é atualizado substituindo o arquivo.
>
> **Diferenças do desktop**: o "importar o estado de login desta máquina" do AutoClaw e do ChatGLM (智谱清言) só existe no Windows (ele lê texto cifrado do DPAPI, então essa seção fica oculta no macOS / Linux); os demais (Raccoon / CatPaw / Cline) leem arquivos em texto puro sob `HOME` e funcionam nas três plataformas. Bandeja, início automático, acesso pela rede local e hospedagem do painel web também funcionam no macOS / Linux.

## Servidor (Windows / Linux, sem interface)

Para **servidores sem interface gráfica** (Windows Server, servidores Linux, instâncias na nuvem). Os artefatos são o mesmo binário `agent2api-server`: sem janela, sem bandeja, sem link com WebView / GTK e sem Node ou qualquer outro runtime em execução. O comando apenas **instala**; a partir daí tudo (contas, chaves, relatórios) acontece no painel no navegador.

O serviço roda como serviço do sistema: inicia no boot, reinicia sozinho após um travamento e não deixa console em segundo plano.

### Windows Server

Baixe `agent2api-server-windows-amd64.zip`, extraia e abra o PowerShell **como administrador**:

```powershell
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg
.\install-service.ps1
```

O script instala o programa em `C:\Agent2API`, coloca os dados em `C:\ProgramData\Agent2API`, registra o serviço do Windows chamado `Agent2API` (`LocalSystem`, inicia no boot), adiciona uma regra de entrada no firewall e o inicia.

```powershell
# Local e porta personalizados
.\install-service.ps1 -InstallDir D:\apps\agent2api -DataDir D:\data\agent2api -Port 8080

# Administração do dia a dia
Get-Service Agent2API
Restart-Service Agent2API
Stop-Service Agent2API

# Desinstalar (mantém os dados); com -Purge apaga programa e dados juntos
.\install-service.ps1 -Uninstall
```

**Atualização**: extraia a versão nova e rode `install-service.ps1` de novo (ele para o serviço, sobrescreve programa e UI e inicia outra vez; dados e configuração ficam intactos).

> **Duas armadilhas específicas do Windows que o script já resolve**:
> o diretório de trabalho de um processo de serviço é `C:\Windows\system32`, não a pasta do exe — o diretório da UI é localizado a partir do caminho do exe, então nada muda; o diretório de dados é gravado explicitamente na entrada de registro do serviço como `AGENT2API_PROXY_HOME` (sem isso os dados cairiam em `C:\Windows\system32\config\systemprofile` do LocalSystem, onde ninguém acharia).
> Um serviço não tem console, então os logs também vão para o arquivo `C:\ProgramData\Agent2API\logs\server.log`.

### Linux (systemd)

```bash
tar -xzf agent2api-server-linux-amd64.tar.gz
cd agent2api-server-linux-amd64      # ou o nome do diretório extraído
sudo bash install.sh
```

O script cria a conta de sistema `agent2api` (sem login), instala o programa em `/opt/agent2api`, coloca os dados em `/var/lib/agent2api`, escreve a configuração em `/etc/agent2api/agent2api.env` e instala a unidade systemd com `enable + start`.

```bash
# Atualizar (mantém dados e configuração; o binário antigo vira .bak)
sudo bash install.sh --upgrade

# Desinstalar (mantém os dados); com --purge apaga dados e configuração juntos
sudo bash install.sh --uninstall

# Administração do dia a dia
systemctl status agent2api
systemctl restart agent2api
journalctl -u agent2api -f
```

Início no boot, reinício automático após travamento (`Restart=always`) e desligamento gracioso no SIGTERM (espera as requisições em andamento, então conexões SSE longas não são cortadas) já vêm configurados. Depois de editar `/etc/agent2api/agent2api.env`, rode `systemctl restart agent2api`.

> **Não ative `AGENT2API_SELF_UPDATE` em máquina física**: aquele é o mecanismo de dentro do contêiner (troca arquivos, encerra e deixa a política de restart do contêiner subir de novo). Em máquina física os arquivos do programa pertencem ao root e a conta do serviço não pode substituí-los; a unidade já o define como `0` explicitamente. Atualize com `install.sh --upgrade`.

### Depois de instalar

As duas plataformas são iguais — abra o navegador:

```
http://<IP do servidor>:3065/
```

1. A primeira visita guia o **cadastro do administrador** (também pode ser predefinido por variável de ambiente, veja abaixo)
2. Depois de entrar, crie uma API key na página "Gateway Keys"
3. Aponte o `base_url` do cliente para `http://<IP do servidor>:3065/v1` e use a chave criada como `api_key`

> ⚠️ **`/v1/*` recusa o serviço enquanto não existir uma chave** (fail-closed). Isso é proposital: a forma servidor escuta em `0.0.0.0`, e expor a cota do upstream para a rede inteira sem chave não é aceitável.

### Implantação sem interação

Tanto o administrador quanto a chave de gateway podem ser predefinidos por variáveis de ambiente, dispensando os passos manuais no navegador:

| Variável | Descrição |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | Predefine o administrador (senha em texto puro, convertida para bcrypt na inicialização); `AGENT2API_ADMIN_PASSWORD_HASH` aceita um hash pronto |
| `AGENT2API_PROXY_API_KEY` | Predefine uma chave de gateway e dispensa "criar a primeira no painel" |
| `AGENT2API_PROXY_HOME` | Diretório de dados |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | Endereço de escuta (padrão `0.0.0.0`) / porta (padrão `3065`) |
| `AGENT2API_PANEL_PORT` | Separa o painel em outra porta: o plano de gerenciamento escuta nela e a porta principal serve apenas `/v1/*` |
| `AGENT2API_ALLOW_NO_KEY` | `1` desativa o fail-closed (somente redes internas) |
| `AGENT2API_LOG_FILE` | Caminho do arquivo de log; padrão `{diretório de dados}/logs/server.log` |
| `AGENT2API_VERBOSE` | `1` ativa o log de debug |

No Linux coloque-as em `/etc/agent2api/agent2api.env`; no Windows edite a chave de registro do serviço `HKLM\SYSTEM\CurrentControlSet\Services\Agent2API\Environment` (o `install-service.ps1` já grava o básico ali; rodar o script de novo sobrescreve).

> **Contas não podem ser predefinidas**: elas exigem leitura de QR code / código por SMS / credenciais coladas, o que é decidido pelos protocolos do upstream — não há como contornar. Para implantação em massa, copie um **diretório de dados** já configurado (o banco SQLite e o `account.key` ficam lado a lado) e as contas vão junto.

## Docker

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

Abra `http://<host>:3065` no navegador: a primeira visita guia o **cadastro do administrador**; depois de entrar, crie uma API key na página "Gateway Keys" para os clientes — `http://<host>:3065/v1` é o endpoint compatível com OpenAI (sem nenhuma chave ele recusa o encaminhamento). Todo o estado (banco SQLite / configuração / logs) fica em um único volume `./data`.

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

Variáveis de ambiente (todas opcionais):

| Variável | Descrição |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | Predefine o administrador e a senha (a senha vai em texto puro e é transformada em hash na inicialização). Sem isso, use o cadastro no painel |
| `AGENT2API_PANEL_PORT` | Painel em porta separada: ao definir, o painel (interface + `/api/*`) escuta só nessa porta e basta mapear a porta principal publicamente para manter o plano de gerenciamento interno (vincule a porta do painel ao loopback, como `127.0.0.1:3066:3066`) |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | Endereço de escuta (padrão `0.0.0.0`) / porta (padrão `3065`) |
| `AGENT2API_ALLOW_NO_KEY` | `1` desativa o fail-closed (serve `/v1` mesmo sem chave configurada — só para redes internas) |
| `AGENT2API_CAPTCHA_ENABLED` | Widget de verificação humana na tela de login: `1` ativado (padrão), `0` desativado |
| `AGENT2API_SELF_UPDATE` | Ponto de atualização dentro do contêiner: `1` ativado por padrão (o painel oferece "Baixar e atualizar"); `0` desativa e deixa apenas a atualização manual |
| `WORKBUDDY_UPDATE_REPO` + `WORKBUDDY_UPDATE_API_BASE` | Avançado: aponte "verificar atualizações / baixar atualizações" para a sua própria fonte. O primeiro troca o repositório (por padrão segue o upstream `Agent2API/agent2api`; em fork, sobrescreva), o segundo troca a raiz da API (padrão `https://api.github.com`; deve devolver um JSON de release no formato do GitHub). Com uma fonte própria configurada, a lista de permissões de download passa a aceitar, além disso, apenas **o próprio host dessa fonte** (o esquema segue a fonte: uma fonte `http://` também permite download em texto puro — só para ensaios locais / espelhos internos) |

Compilar a partir do código: clone o repositório e rode `docker compose up -d --build` (a imagem contém apenas o gateway e o painel, sem toolchain Rust).

**Diferenças de recursos no painel web** (decorrem de "não haver cliente de desktop local"): login pela web (WorkBuddy / Qoder / Cline), código por SMS e credenciais coladas funcionam plenamente; logins pela web que exigem callback nesta máquina (AutoClaw / CatPaw / Accio / CodeArts / Trae) e a "importação do estado de login local" não estão disponíveis — use credenciais coladas. Com o desktop e a opção de expor o painel ativadas, abrir no navegador mostra as mesmas diferenças: uma interface, um conjunto de regras.

### Atualização dentro do contêiner (opcional)

A página "Atualização de software" do painel consegue baixar e aplicar atualizações de dentro do contêiner: busca `agent2api-server-linux-<arch>.tar.gz` no Release oficial (validando o `.sha256`), substitui o executável e a pasta `ui/` e encerra o processo de forma graciosa, para que a política de `restart` suba o mesmo contêiner de novo. Controlado por `AGENT2API_SELF_UPDATE` (`1` no Dockerfile); defina `0` para atualizar apenas manualmente.

O mesmo `agent2api-server-linux-<arch>.tar.gz` também é o pacote de instalação do [servidor Linux](#linux-systemd): os itens `agent2api-server` e `ui/` são exatamente o que a cadeia de atualização lê pelo nome, e ele ainda traz `install.sh` / `agent2api.service` / `agent2api.env.example`. A cadeia de atualização lê apenas os dois primeiros (o resto é descartado — veja `selfupdate::extract_bundle`), então um artefato serve aos dois propósitos: extrair e substituir dentro do contêiner, extrair e rodar `install.sh` em máquina física.

Atenção ao custo: depois de uma atualização interna, a tag da imagem e a versão realmente em execução **divergem** (`docker inspect` continua mostrando a tag antiga), e `docker compose pull && up -d` recria o contêiner na versão embutida na imagem. Se você quer versões fixas, deixe esse caminho desativado.

## Verificação e conexão do cliente

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

## Acesso pela rede local

Por padrão o gateway escuta apenas `127.0.0.1`. Ao ativar "Configurações → Geral → Acesso pela rede local" ele passa a escutar todos os adaptadores e os dispositivos da rede só precisam apontar o endereço da API para o IP desta máquina (a interface mostra o endereço completo). Por segurança, é obrigatório registrar um administrador do painel antes de ativar: a API de gerenciamento passa a exigir sessão de administrador ou uma Gateway Key e, sem chave ativa, a API de encaminhamento também recusa (o fluxo cria uma chave "padrão" automaticamente). O painel web pode ser exposto à rede local opcionalmente (fechado por padrão). As mudanças valem após reiniciar.

Os instaladores de desktop (Windows / macOS) já incluem a pasta `ui/` do painel web: ao ativar também a opção de expor o painel de gerenciamento à rede local, dispositivos da rede podem abrir o mesmo endereço no navegador e ver a interface de gerenciamento — a mesma interface da versão Docker. **A janela do desktop em si nunca passa pelo login web** (não existe página de login no app): ela se autentica com a primeira Gateway Key desta máquina, então ligar esse interruptor não pede login no app. A página de login web só aparece quando um navegador abre o painel.

## Celular

Há duas formas — escolha a que precisar:

| Forma | Como instalar | Indicada para |
| --- | --- | --- |
| **Navegador / PWA** | Abra `/m` no navegador do celular; opcionalmente "Adicionar à tela inicial" | Consulta rápida, sem instalar nada |
| **APK Android** | Instale `Agent2API-<versão>.apk` | Ícone na tela inicial, tela cheia, vários servidores |

As duas rodam o mesmo código de interface. A diferença é quem serve a página: o PWA é servido pelo gateway (então "também expor o painel de gerenciamento web" precisa estar ligado), enquanto o APK leva a interface dentro do pacote (então consegue abrir a interface offline e não precisa dessa opção).

### Pré-requisitos (por forma)

**Desktop (Windows / macOS / Linux)**: as duas opções precisam estar ligadas; uma sem a outra não funciona.

1. "Configurações → Geral → Acesso pela rede local" — permite que dispositivos da rede acessem o gateway. O fluxo de ativação pede o registro de um administrador do painel e cria automaticamente uma chave chamada "padrão".
2. "Configurações → Geral → Acesso pela rede local → também expor o painel de gerenciamento web" — serve a pasta estática `ui/`. **Necessária para a forma navegador / PWA** (`/m` é servido pelo gateway, então retorna 404 enquanto esta opção estiver desligada). **O APK não precisa dela** (a interface está no pacote), mas ainda precisa da opção 1 — sem ela o gateway escuta apenas em `127.0.0.1` e o celular não alcança.

Ambas exigem reinício para valer.

**Servidor / Docker**: nenhuma opção extra — as duas formas já escutam em todos os adaptadores e servem `ui/` por padrão (`AGENT2API_UI_DIR=/app/ui` está fixo no Dockerfile). Basta que a porta do contêiner ou do serviço seja alcançável pela rede local. Se você definiu `AGENT2API_HOST=127.0.0.1` manualmente, mude para `0.0.0.0`; se `AGENT2API_UI_DIR` apontar para um diretório inexistente, o painel retorna 404 mas a API continua funcionando.

### Abrir no celular (navegador / PWA)

`http://<IP da rede local da máquina que roda o gateway>:3065/m`

No desktop, é o endereço mostrado no painel de configurações acima (por exemplo `192.168.1.23`) — troque o `/v1` final por `/m`. Para Docker / servidor use o IP da rede local do host e a porta mapeada.

A primeira visita redireciona para a página de login; entre com o administrador do painel e ela volta automaticamente para a interface móvel. Se ainda não houver administrador, a página de login pede para criar um.

### Instalar o APK (Android)

Na primeira execução aparece uma tela "Servidores". São três passos:

1. Toque em "Adicionar" e informe o `IP:porta` do gateway (por exemplo `192.168.1.23:3065`; a porta padrão é 3065);
2. Toque em "Conectar e salvar" — ele testa o gateway antes, então endereço errado ou gateway sem acesso pela rede local são reportados ali mesmo;
3. Aparece a página de login. Informe a conta de administrador e a senha deste gateway (se ainda não houver administrador, vira "Criar conta de administrador").

**Vários servidores**: toque em "Adicionar" de novo. Cada servidor guarda o próprio login — trocar não exige entrar de novo, e voltar também não. O ponto de entrada é "Configurações → Servidores"; toque em uma linha para trocar.

**Limitações conhecidas do APK**:

- Apenas Android. O iOS exige uma conta Apple Developer para assinar, ou instalação pelo Xcode (assinaturas de sideload expiram em 7 dias).
- Fala com endereços `http://` da rede local em texto claro. O risco é "interceptável na mesma rede local"; o gateway em si continua exigindo login de administrador. Para eliminar, configure TLS no gateway e informe um endereço `https://` (a análise é a mesma, apenas não prefixa `http`).
- A interface segue a versão do APK, não a do gateway. Se uma atualização do gateway adicionar telas novas, é preciso um APK mais novo para vê-las; mudanças apenas de dados não são afetadas.

### Adicionar à tela inicial (navegador / PWA)

Opcional — dá tela cheia sem barra de endereço. A interface móvel oferece o ponto de entrada em "Configurações → Instalar como app":

- **Android / Chrome**: toque em "Adicionar à tela inicial" nas configurações (abre o diálogo de instalação do sistema); ou use o menu → "Adicionar à tela inicial" / "Instalar app"
- **iOS / Safari**: o Safari não tem API de instalação, então a página de configurações mostra o passo a passo — Compartilhar → "Adicionar à Tela de Início"

Abrir pelo ícone da tela inicial leva direto à página inicial móvel. Quando já está rodando numa janela independente, a página de configurações mostra "Instalado".

Quando há uma nova versão, uma barra "Reiniciar e atualizar" surge na parte de baixo: após a atualização do gateway a nova interface já foi baixada e um toque alterna para ela (ignorar não atrapalha o uso atual — passa a valer na próxima abertura).

> **Por que não há app nativo nem pacote na loja**: a interface móvel sempre fala com um gateway rodando em **outra máquina** (o gateway precisa ler os logins dos clientes daquela máquina e gravar seus arquivos de configuração — não pode rodar num celular). É fundamentalmente "acesso remoto a um serviço HTTP", e o navegador é o veículo certo: autenticação de mesma origem, sem instalação, e atualiza junto com o gateway. A forma PWA acrescenta tela cheia e ícone na tela inicial, então parece um app nativo.
>
> **Service Worker (shell offline) só funciona em `https` ou `localhost`**: o acesso pela rede local usa `http://<IP>`, onde os navegadores não oferecem SW, então a interface móvel não abre offline. É uma restrição de segurança do navegador, não um recurso ausente — no uso normal o gateway está online, então não faz diferença.

### O que pode e não pode fazer

| Pode | Não pode (use o app de desktop) |
| --- | --- |
| Relatórios, status das contas, saldos, logs de requisições, logs de execução | Adicionar contas (QR / colar token / importar login do cliente) |
| Ativar/desativar contas, atualizar credenciais, excluir contas | Editar credenciais, proxy ou regras de limite de uma conta |
| Check-in com um toque, ver histórico de check-in | Resgatar recompensas de integração e planos individualmente |
| Ativar/desativar modelos, filtrar por provedor | Editar mapeamentos de modelo, níveis de raciocínio, flags de capacidade |
| Copiar gateway keys, ativar/desativar keys | Criar keys, definir listas de provedores/modelos permitidos |
| Ativar/desativar proxies, testar conectividade | Adicionar proxies, gerenciar assinaturas, baixar o kernel |
| Ativar/desativar tarefas agendadas, alterar intervalos | Alterar porta, roteamento, retry, timeouts, retenção de dados |
| Consultas rápidas do assistente (saldos / modelos duplicados / uso de hoje / contas limitadas) | Conversa livre e chamada de ferramentas do assistente |
| Ver status do takeover de Agent, restaurar | Reescrever configurações de clientes (um celular não lê arquivos locais) |

A barra lateral do desktop tem uma entrada "Abrir no celular" no rodapé que leva à mesma página.
