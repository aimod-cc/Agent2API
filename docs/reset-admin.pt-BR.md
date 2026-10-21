# Redefinir o administrador do painel (senha esquecida)

[简体中文](./reset-admin.md) | [English](./reset-admin.en.md) | [繁體中文](./reset-admin.zh-Hant.md) | [日本語](./reset-admin.ja.md) | [한국어](./reset-admin.ko.md) | **Português (BR)**

Esta página trata de uma situação: **você esqueceu a senha do administrador do painel e não consegue entrar**.

A senha é armazenada apenas como hash bcrypt, então não há como "recuperá-la" — o único
caminho é **excluir** o registro do administrador, o que devolve o painel ao estado
"não registrado" para você criar outro. Isso apaga a conta e a senha do administrador,
mas não afeta o pool de contas, as chaves do gateway, os logs nem qualquer outro dado
de negócio.

> Se você ainda consegue entrar, não use esta página: vá em
> **Configurações → Conta → Alterar senha**. Esse fluxo exige a senha atual e revoga
> automaticamente os acessos em outros dispositivos.

## 1. Pare o programa

O registro precisa ser excluído com o programa **totalmente parado**. O banco roda em
modo WAL; gravar enquanto ele está em execução pode ser sobrescrito ou bloqueado.

| Forma | Como parar |
| --- | --- |
| Desktop Windows | Clique com o botão direito no ícone da bandeja → **Sair** (fechar a janela não basta: por padrão ela vai para a bandeja e o processo continua) |
| Desktop macOS / Linux | Ícone da bandeja / barra de menus → **Sair** (no macOS, `Cmd+Q` e confirme que o ícone desapareceu) |
| Servidor Windows | PowerShell como administrador: `Stop-Service Agent2API` |
| Servidor Linux | `sudo systemctl stop agent2api` |
| Docker | No diretório do compose: `docker compose stop` |

## 2. Localize o arquivo do banco

O banco é um único arquivo SQLite, `agent2api.db`, dentro do "diretório de configuração":

| Forma | Caminho |
| --- | --- |
| Windows (desktop / servidor) | `%USERPROFILE%\.agent2api\agent2api.db` |
| macOS / Linux (desktop / servidor) | `~/.agent2api/agent2api.db` |
| Docker (padrão do compose) | `./data/agent2api.db` no host (`/data/agent2api.db` no contêiner) |

Se `AGENT2API_PROXY_HOME` estiver definido, o diretório de configuração é esse valor.

**Configurações → Informações da versão → Arquivo de dados** mostra o caminho completo
enquanto você ainda consegue entrar.

## 3. Exclua o registro do administrador

O administrador fica na tabela `kv`, com a chave **`panelAdmin`**.

### Com `sqlite3`

```bash
# Desktop e servidor em Windows / macOS / Linux
sqlite3 ~/.agent2api/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"

# Formato de caminho do Windows (PowerShell ou CMD)
sqlite3 "%USERPROFILE%\.agent2api\agent2api.db" "DELETE FROM kv WHERE key='panelAdmin';"

# Docker: pare primeiro, depois exclua no diretório data do host
docker compose stop
sqlite3 ./data/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"
```

### Sem `sqlite3`

Qualquer ferramenta gráfica de SQLite serve (DB Browser for SQLite, DBeaver, Navicat…):
abra `agent2api.db` → abra a tabela `kv` → exclua a linha cujo `key` é `panelAdmin` →
salve / grave as alterações.

Python também funciona (geralmente já vem instalado):

```bash
python -c "import sqlite3,os; p=os.path.expanduser('~/.agent2api/agent2api.db'); c=sqlite3.connect(p); c.execute(\"DELETE FROM kv WHERE key='panelAdmin'\"); c.commit(); c.close(); print('done')"
```

## 4. Reinicie e registre novamente

Reinicie o programa (`docker compose start` no Docker) e abra o painel:

- **Desktop**: a janela vai direto para a interface principal (sem administrador = sem
  login). Para definir uma senha novamente, abra **Configurações → Conta** — ela indicará
  a página de login para o registro; você também pode abrir `http://127.0.0.1:3065/login`
  diretamente.
- **Servidor / Docker**: abrir o painel redireciona para a página de login, que mostra
  **"Primeiro uso: crie uma conta de administrador"**. Preencha usuário e senha.

Depois do registro, o painel volta a exigir login.

## 5. Atenção: variáveis de ambiente valem só na primeira vez

Se a implantação define:

```bash
AGENT2API_ADMIN_USER=admin
AGENT2API_ADMIN_PASSWORD=your-password
```

elas são usadas **apenas como semente inicial** — gravadas uma vez, quando o banco ainda
não tem administrador; depois disso o banco é a fonte da verdade (é isso que permite
alterar a senha pelo painel). Portanto:

- Alterar essas variáveis **não** altera uma senha já registrada;
- Após excluir `panelAdmin` e reiniciar, se as variáveis ainda existirem elas **criam um
  novo administrador** — nesse caso a "nova senha" é a que está nas variáveis.

Remova as variáveis da configuração de implantação antes de reiniciar se não quiser que a
conta seja recriada automaticamente.

`AGENT2API_ADMIN_PASSWORD_HASH` (hash bcrypt, tem precedência sobre a senha em texto
puro) se comporta da mesma forma.

## 6. Aviso de segurança

Depois de excluir o registro, o painel fica **sem autenticação** no estado "não registrado":

- Desktop: só é acessível nesta máquina, então o risco é limitado — mas, se
  "permitir acesso pela LAN" estiver ativo, outros dispositivos da rede também abrem o
  painel. Registre-se novamente o quanto antes.
- Servidor / Docker: **qualquer pessoa que alcance a porta do painel pode assumir a área
  de administração**. Reinicie e registre imediatamente; não permaneça nesse estado.
  Isso é especialmente crítico quando a porta de administração está exposta à internet.

Se você danificar o banco por engano, pare: pode haver um diretório `backups/` ao lado
dele (snapshots de segurança criados antes de cada restauração completa) — copie um
`pre-restore-*.db` por cima do arquivo para recuperar.
