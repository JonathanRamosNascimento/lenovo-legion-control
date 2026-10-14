# Legion Control

Alternativa nativa para Linux ao Lenovo Vantage / Legion Toolkit, escrita em
Rust, com foco em **segurança**: só usa interfaces documentadas do kernel
(sysfs, platform_profile, firmware-attributes, power-profiles-daemon, NVML),
valida todo valor contra o hardware real e **nunca** escreve no Embedded
Controller nem chama métodos ACPI/WMI diretamente.

Componentes:

| Binário | Função |
|---|---|
| `legionctl` | CLI: diagnóstico, monitoramento e controle |
| `legion-control-daemon` | serviço D-Bus privilegiado (`com.legion.Control`), autorizado por polkit |
| `legion-control` | interface GTK4/libadwaita + ícone na bandeja |

## Hardware suportado

| Modelo | Estado |
|---|---|
| Lenovo Legion 5 15IAX10 (83F0, board LNVNB161216) — Core Ultra 7 255HX, RTX 5070 Laptop | Referência |
| Outros Legion (5, 7, Pro, Slim) | Somente interfaces genéricas, detectadas em tempo de execução |

Detalhes por recurso e a origem de cada interface:
[docs/hardware-support.md](docs/hardware-support.md).

## Recursos

| Recurso | Interface | Estado |
|---|---|---|
| Perfis (quiet/balanced/performance/extreme/custom) | power-profiles-daemon → `platform_profile` | ✅ |
| Monitoramento CPU/GPU/RAM/temperaturas/ventoinhas/bateria | sysfs, procfs, NVML | ✅ |
| Modo de conservação da bateria, tipo de carga | `conservation_mode` (ideapad), `charge_types` | ✅ |
| Limite numérico de carga | `charge_control_end_threshold` | ✅ onde existir (não no 15IAX10) |
| Governor, EPP, turbo | cpufreq / intel_pstate | ✅ |
| Controle de ventoinha (fixo e curva) | `lenovo_wmi_other` `fanN_target` | ⚠️ experimental, desligado por padrão |
| Limites de potência/temperatura (PL1, PL2, cTGP…) | firmware-attributes `lenovo-wmi-other` | ⚠️ experimental, desligado por padrão, só no perfil *custom* |
| Perfis personalizados | `~/.config/legion-control/profiles/*.json` | ✅ |
| Cor e brilho do teclado | protocolo proprietário do controlador ITE (Spectrum), com backup do efeito original | ⚠ experimental, desligado por padrão |
| Troca de MUX | — | ❌ não suportado |

## Instalação (Arch / CachyOS)

```sh
cd packaging/arch/legion-control
makepkg -si                      # gera e instala legion-control-*.pkg.tar.zst
sudo systemctl enable --now legion-control.service
```

Ou instale um pacote já gerado com
`sudo pacman -U legion-control-*.pkg.tar.zst`. A versão VCS está em
`packaging/arch/legion-control-git` (defina `LEGION_CONTROL_GIT` com a URL
do repositório).

O pacote instala os binários, a unit systemd, a política D-Bus, as ações
polkit, `/etc/legion-control/daemon.json`, o `.desktop` e o ícone.

Opcionais: `power-profiles-daemon` (recomendado), `nvidia-utils` (NVML),
`gnome-shell-extension-appindicator` (bandeja no GNOME).

### Início automático

* O daemon é iniciado pelo systemd (`systemctl enable`).
* A GUI: menu ☰ → **Start at Login**, que cria
  `~/.config/autostart/legion-control.desktop` (`legion-control --hidden`,
  inicia só na bandeja).

## Idioma

A interface está disponível em **inglês** e **português (Brasil)**.

* **GUI**: menu ☰ → **Idioma** → Automático / English / Português (Brasil).
  A troca é imediata e fica salva em `~/.config/legion-control/gui.json`.
* **CLI**: `legionctl --lang pt-BR <comando>`, ou a variável
  `LEGION_LANG=pt-BR`; sem nenhum dos dois, segue o `LANG` do sistema
  (`pt_BR.UTF-8` → português).
* A saída `--json` e os logs do daemon ficam sempre em inglês (formato
  estável para scripts); mensagens vindas do daemon são traduzidas pela
  CLI/GUI.
* Diálogos de autenticação (polkit) e o atalho do menu também têm tradução.

Traduções ficam em `crates/legion-i18n/src/catalog.rs` (pares
inglês → português, com modelos `{}` para mensagens com valores).

## Modo Personalizado (estilo Legion Space)

A página **Personalizado** da GUI reúne os ajustes do modo custom do
Legion Space/Vantage: limites de potência da CPU (PL1 longo prazo, PL2 curto
prazo, carga cruzada, Tau), limite de temperatura da CPU, e na GPU cTGP,
Dynamic Boost, offset na tomada e limite de temperatura, cada um com slider
e campo numérico, mais o atalho para a curva das ventoinhas.

* Faixas, passos e padrões vêm do firmware em tempo de execução
  (`lenovo-wmi-other`); atributos sem faixa informada aparecem como "fixado
  pelo firmware".
* As alterações são aplicadas juntas (uma autenticação), só no perfil
  Personalizado, com PL1 ≤ PL2 garantido e ordem de escrita segura.
* Ao trocar para outro perfil, o firmware restaura os valores dele; salve
  seus ajustes em **Perfis** para reaplicar depois.

## Configuração

`/etc/legion-control/daemon.json` (root):

```json
{
  "enable_fan_control": false,
  "enable_power_limits": false,
  "fan_safety_cpu_c": 95.0,
  "fan_safety_gpu_c": 85.0
}
```

* `enable_fan_control` — permite velocidade fixa e curvas. O daemon devolve
  o controle ao firmware (alvo 0) ao atingir as temperaturas de segurança,
  se o sensor da CPU sumir, após 3 falhas de escrita e ao parar.
* `enable_power_limits` — permite escrever PL1/PL2/limites térmicos
  (também pode ser ligado pela GUI, página **Personalizado**, ou com
  `legionctl config set enable_power_limits on`; ambos pedem a senha de
  administrador). Pela
  documentação do kernel, **só têm efeito no perfil `custom`** e o BIOS os
  sobrescreve ao trocar de perfil.

* `enable_keyboard_rgb` — permite mudar a cor e o brilho do teclado (GUI,
  página **Teclado**, ou `legionctl config set enable_keyboard_rgb on`; pede a
  senha de administrador). Usa um protocolo proprietário do controlador do
  teclado, sem documentação da Lenovo: veja a seção **Teclado** abaixo e
  [docs/hardware-support.md](docs/hardware-support.md). Desligado, a interface
  do teclado nem é consultada.

Reinicie o daemon após editar: `sudo systemctl restart legion-control`.

## Teclado (cor e brilho) — experimental

```sh
legionctl config set enable_keyboard_rgb on      # uma vez (senha de administrador)
legionctl keyboard                               # estado: efeito, perfil, brilho
legionctl keyboard color '#ff8000'               # ou: red, blue, purple… (aspas por causa do #)
legionctl keyboard brightness 5                  # 0 (apagado) a 9
legionctl keyboard restore                       # devolve o efeito que o teclado tinha
```

* A cor é uma **cor fixa em todo o teclado**, gravada no **perfil ativo** do
  próprio teclado (sobrevive a reboots; nada é reaplicado no boot). Efeitos,
  zonas e animações não são suportados.
* Antes da primeira alteração, o efeito original do perfil é salvo em
  `/var/lib/legion-control/` e `keyboard restore` o recoloca (funciona mesmo
  com o recurso desligado depois).
* Cada alteração grava na memória flash do controlador: use-a para
  configurar, não para animar. Nada é gravado se o teclado já mostra a cor.
* Hoje só o controlador `048d:c195` (Legion 5 15IAX10) está na lista
  verificada; outros modelos aparecem como "detectado, não usado".
* GUI: página **Teclado** (ativar, cor, predefinições, brilho, restaurar).

Permissões (polkit): perfil e bateria são liberados para o usuário da
sessão ativa; CPU, ventoinhas, limites de potência e teclado pedem senha de
administrador.

## Comandos

```sh
legionctl status | doctor | hardware | capabilities | hardware-scan [-o arq]
legionctl profile [quiet|balanced|performance|max-power|custom]
legionctl cpu status | epp <valor> | governor <valor> | turbo on|off
legionctl gpu status [--wake-gpu]
legionctl fan status | auto | fixed <rpm> | curve <arquivo.json> | curve --example
legionctl battery [status] | conservation on|off | charge-type <tipo> | threshold <n>
legionctl power-limit [list] | set <nome> <valor>
legionctl keyboard [status] | color <#rrggbb|nome> | brightness <0-9> | restore
legionctl config [show] | set enable_fan_control|enable_power_limits|enable_keyboard_rgb on|off
legionctl custom list | show <nome> | save <nome> [-d desc] | import <nome> <arq> | apply <nome> | delete <nome>
legionctl thermal | daemon status | version
legionctl --json <comando>
```

Leituras funcionam sem privilégios e sem daemon. Alterações vão pelo daemon
(D-Bus + polkit); sem daemon, `legionctl` escreve diretamente se rodar como
root (e perfis quiet/balanced/performance funcionam via
power-profiles-daemon mesmo sem root).

Exemplo de perfil personalizado:

```json
{
  "description": "Jogos na tomada",
  "platform_profile": "custom",
  "power_limits": { "ppt_pl1_spl": 95, "ppt_pl2_sppt": 150 },
  "cpu_epp": "performance",
  "battery_conservation": true,
  "fan_mode": { "mode": "curve", "points": [
      { "temp_c": 55, "rpm": 2200 }, { "temp_c": 70, "rpm": 3200 },
      { "temp_c": 85, "rpm": 4600 } ], "hysteresis_c": 4 }
}
```

Campos desconhecidos são rejeitados; cada valor é validado contra o
hardware antes de qualquer escrita.

## Troubleshooting

| Sintoma | Causa / solução |
|---|---|
| GUI: "Daemon unavailable" | `sudo systemctl enable --now legion-control.service`; veja `journalctl -u legion-control` |
| "Administrator permission required." | sem daemon e sem root; ou polkit negou (verifique o agente de autenticação) |
| "manual fan control is disabled" | habilite `enable_fan_control` em `daemon.json` |
| "switch to the custom profile first" | limites de potência só funcionam em `legionctl profile custom` |
| Ventoinhas voltaram para automático sozinhas | proteção térmica; veja `legionctl daemon status` (Safety event) |
| EPP volta ao valor anterior | power-profiles-daemon reaplica o EPP ao trocar de perfil |
| `GPU monitoring ... suspended` | a dGPU está em economia; NVML não é consultado para não acordá-la (`--wake-gpu` força) |
| Sem ícone na bandeja no GNOME | instale/ative a extensão AppIndicator; a janela funciona sem bandeja |
| NVML falha dentro do serviço | o sandbox só libera `/dev/nvidiactl` e `/dev/nvidia0`; ajuste `DeviceAllow` se seu sistema usar outros nós |

Pacote de diagnóstico: `scripts/collect-debug.sh`. Logs:
`RUST_LOG=debug legionctl doctor`, `journalctl -u legion-control`
(alterações de hardware aparecem com o alvo `legion_audit`).

## Segurança

* Uma única primitiva de escrita (`legion_core::sysfs::write_attr`): só
  aceita caminhos sob `/sys`, nunca cria arquivos, registra valor antigo e
  novo e relê o valor.
* Todo valor é validado contra o que o kernel anuncia (choices, min/max,
  incremento, listas de EPP/governor, faixa de RPM).
* Só drivers/atributos documentados são escritos (lista explícita de
  atributos de `lenovo-wmi-other`; ventoinha só no chip `lenovo_wmi_other`).
* Nenhum comando de shell, nenhum caminho vindo do usuário/GUI/D-Bus.
* Teclado: só um dispositivo USB não-removível da lista verificada, com o
  descritor HID esperado; relatórios sempre com o tamanho que o descritor
  declara; backup do efeito original antes de gravar.
* Daemon com polkit por ação, sandbox systemd (sem capabilities, sem rede,
  FS somente leitura exceto `/sys` e `/var/lib/legion-control`), recursos limitados.
* A GUI nunca acessa `/sys`: só D-Bus.
* Números de série, UUIDs e MACs nunca são lidos.

## Limitações

* Escritas em hardware real foram validadas por testes com árvores `/sys`
  falsas e pela documentação do kernel; controle de ventoinha e limites de
  potência continuam **experimentais** até serem confirmados no hardware.
* Modo gráfico (MUX) apenas detectado.
* Teclado: só cor fixa + brilho, via protocolo proprietário (experimental). O
  padrão HID LampArray existe no controlador, mas é inerte neste firmware.
* Qual ventoinha é CPU/GPU não é informado pelo driver.

## Desenvolvimento

Workspace com 15 crates — veja [docs/architecture.md](docs/architecture.md)
e [docs/development.md](docs/development.md).

```sh
cargo test --workspace && cargo clippy --workspace --all-targets
```

## Licença

MIT — veja [LICENSE](LICENSE). Nenhum código de terceiros (ex.:
LenovoLegionLinux, GPL-2.0) foi copiado; o comportamento de hardware é
documentado com a sua fonte.
