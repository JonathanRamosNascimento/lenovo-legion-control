# Suporte de hardware

Legion Control **nunca assume** que um recurso existe: tudo é detectado em
tempo de execução (`legionctl capabilities`). Esta página registra o que foi
**observado** em hardware real e a **fonte** de cada comportamento. Dois
modelos Legion nunca são tratados como iguais.

Legenda: **R** = monitorado · **W** = controle implementado · **W⚠** =
controle experimental, desligado por padrão em `/etc/legion-control/daemon.json`
· **—** = não existe.

## Lenovo Legion 5 15IAX10

| Campo | Valor observado |
|---|---|
| Machine type (DMI `product_name`) | `83F0` |
| Board (DMI `board_name`) | `LNVNB161216` |
| BIOS | `S2CN26WW` (03/24/2026), EC 1.22 |
| CPU | Intel Core Ultra 7 255HX (8 P + 12 E) |
| GPU | Intel Arrow Lake-S (i915/xe) + NVIDIA GeForce RTX 5070 Laptop GPU (`10de:2d18`) |
| Kernel de referência | 7.2.8-1-cachyos |

| Recurso | Estado | Interface | Fonte |
|---|---|---|---|
| Perfis de energia | R/W | power-profiles-daemon (`ActiveProfile`) para quiet/balanced/performance; `/sys/firmware/acpi/platform_profile` para max-power/custom | `Documentation/ABI/testing/sysfs-platform_profile`; `Documentation/wmi/devices/lenovo-wmi-gamezone.rst` |
| Mapeamento | — | quiet→`low-power` (LED azul), balanced (branco), performance (vermelho), max-power = "Extreme" (roxo), custom (roxo) | lenovo-wmi-gamezone.rst; o driver só oferece `max-power` quando o BIOS o suporta |
| CPU: uso, freq., temp., RAPL | R | `/proc/stat`, cpufreq, coretemp, `intel-rapl:0` | ABI do kernel |
| CPU: governor, EPP, turbo | R/W | `scaling_governor`, `energy_performance_preference`, `intel_pstate/no_turbo` | `Documentation/admin-guide/pm/intel_pstate.rst` |
| Limites de potência CPU (PL1/PL2/cross-load/temp.) | R/W⚠ | firmware-attributes `lenovo-wmi-other-0`: `ppt_pl1_spl` 50–105, `ppt_pl2_sppt` 60–168, `ppt_cpu_cl` 30–65, `cpu_temp` 85–105; `ppt_pl1_tau` fixo | `Documentation/wmi/devices/lenovo-wmi-other.rst` — **só efetivos no perfil custom**; trocar de perfil faz o BIOS sobrescrevê-los |
| Limites GPU | R/W⚠ | `gpu_nv_ac_offset` 10–80, `gpu_temp` 75–87; `gpu_nv_ctgp`, `gpu_nv_ppab`, `gpu_nv_cpu_boost` fixos (min=max=0) | idem |
| GPU NVIDIA | R | NVML (driver 615.71.09 open); só consultado com a dGPU ativa | — |
| Modo gráfico (MUX) | R (inferido) | conector eDP conectado em `card*` | — |
| Temperaturas | R | coretemp, nvme, mt7925, thermal zones; `SEN1..SEN7` sem localização documentada | — |
| Ventoinhas (RPM) | R | hwmon `lenovo_wmi_other` `fan1`/`fan2`, 1800–4600 RPM, divisor 100 | lenovo-wmi-other.rst |
| Controle de ventoinha | W⚠ | `fanN_target` (RPM alvo, **0 = automático**, arredondado ao múltiplo de `fanN_div`) | lenovo-wmi-other.rst |
| Curva de ventoinha | W⚠ | curva em software no daemon sobre `fanN_target`, com histerese e proteção térmica (padrão 95 °C CPU / 85 °C GPU) | — |
| Bateria | R | `BAT0` (BYD L24B4PC0, 80 Wh) | `sysfs-class-power` |
| Modo de conservação | R/W | `VPC2004:00/conservation_mode` (preferido); `charge_types` `Long_Life` | `sysfs-platform-ideapad-laptop`, `sysfs-class-power` |
| Tipo de carga | R/W | `charge_types`: Fast / Standard / Long_Life | `sysfs-class-power` |
| Limite numérico de carga | — | sem `charge_control_end_threshold` | — |
| Cor/brilho do teclado | W⚠ | controlador USB ITE `048d:c195` ("ITE Device(8258)"), protocolo proprietário **Spectrum** (ver [Teclado](#teclado-iluminação)); nenhum LED class de teclado | engenharia reversa da comunidade (LenovoLegionToolkit, MIT) + leituras e escritas verificadas nesta máquina |

**`max-power` e o power-profiles-daemon:** o ppd não conhece `max-power`. Ao
gravá-lo em `platform_profile`, o ppd registra `should not be reached` e,
alguns segundos depois, escreve o EPP dele (`balance_power` na bateria). Por
isso o daemon, depois de trocar para o modo Extremo, mantém o EPP em
`performance` (corrige a cada ciclo) até o firmware sair de `max-power`, ou
até você escolher outro EPP ou aplicar um perfil salvo. Troca feita por
`Fn+Q` não passa pelo daemon e não recebe essa correção.

Drivers de kernel observados: `lenovo_wmi_gamezone`, `lenovo_wmi_other`,
`lenovo_wmi_capdata`, `lenovo_wmi_events`, `lenovo_wmi_hotkey_utilities`,
`ideapad_acpi`, `ideapad_wmi`. 29 blocos WMI sem driver são listados no
relatório sem interpretação e **nunca** são usados.

## Teclado (iluminação)

O teclado interno é um controlador **ITE 8258** (`048d:c195`, USB, declarado
não-removível pelo firmware: `removable=fixed`) com **duas interfaces HID**:

| Interface | Nó (nesta máquina) | Descritor |
|---|---|---|
| 0 | `hidraw5` | teclado + relatórios proprietários: página `0xff89`, relatórios `0xCC` (32 B), `0x5A` (15 B) e `0x07` (**959 B** de payload) |
| 1 | `hidraw6` | **HID LampArray** padrão (página `0x59`, relatórios 1–6), 24 lâmpadas RGB + intensidade, uma fileira de 24 zonas verticais |

### LampArray padrão — testado e **inerte** neste firmware

O padrão USB-IF (HUT 1.4) é a interface ideal, e o código foi escrito e
testado contra o descritor real. No hardware, porém, todas as
transferências são aceitas sem erro e **nenhuma muda a iluminação**,
mesmo com: handshake de descoberta (atributos → atributos de cada lâmpada →
`AutonomousMode=0`), re-enumeração USB do controlador, `intensity` 0/1/255,
atualizações por intervalo (`0x05`) e por lote (`0x04`) com
`LampUpdateComplete`, e polaridade invertida de `AutonomousMode`. (BIOS
`S2CN26WW`, kernel 7.2.) O `AutonomousMode` também não é legível (EPIPE).
Por isso o LampArray **não** é usado.

### Protocolo Spectrum (interface 0, relatório 7) — experimental

Documentação pública da Lenovo: **nenhuma**. Os formatos vêm da engenharia
reversa feita pela comunidade no *LenovoLegionToolkit* (MIT,
`LenovoLegionToolkit.Lib/Native.cs`) e foram confirmados nesta máquina:

* Todo pacote é um *feature report* `7` de 960 bytes (1 de id + 959 — o
  tamanho exato que o descritor declara; só é enviado se o descritor bater).
  Requisição: `07 <op> C0 03 <param> …`; resposta: `07 <op> <tam> 00 <payload>`
  (curta: 5–7 bytes nas consultas simples).
* Operações usadas: compatibilidade `0xD1`, brilho `0xCD`/`0xCE` (0–9),
  perfil ativo `0xCA`, troca de perfil `0xC8`, layout `0xC4`, leitura de efeito
  `0xCC`, escrita de efeito `0xCB`.
* O teclado guarda **6 perfis** de iluminação na própria memória (flash); a
  cor e o brilho **sobrevivem a reboots**, então nada é reaplicado no boot.
* Um efeito é `06 01 <tipo> 02 <vel> 03 <sentido-horário> 04 <direção> 05
  <modo-de-cor> 06 00`, seguido de lista de cores (`n` + RGB) e lista de
  teclas (`n` + `u16`). Cor fixa = tipo `11` (`Always`), 1 cor, lista de
  teclas. Neste modelo "todas as teclas" é a lista explícita `1..24` (o
  código `0x65` do LenovoLegionToolkit não aparece).
* Perfis observados: 1 = onda arco-íris, 3 = áudio, 4/5 = suave, 6 = **luzes
  apagadas** (tipo 255). Os pedidos dos perfis 2 e 5 devolveram o conteúdo dos
  perfis 1 e 4.

**Armadilha confirmada em hardware:** pedir o efeito de um perfil
(`0xCC <n>`) também **ativa** esse perfil — ler o perfil 6 apagou o teclado.
Por isso o código só lê o perfil **ativo** (nenhuma leitura troca de
perfil), e a leitura "bruta" de diagnóstico recusa `0xCC`.

Verificado nesta máquina (Legion 5 15IAX10, via `Keyboard`/`spectrum-ctl`):
compatibilidade, brilho, perfil ativo, layout (`9 × 22`), leitura do efeito
ativo, brilho 3 ↔ 9, cor fixa vermelho e azul, e **restauração byte a byte**
do efeito original (arco-íris).

Salvaguardas no Legion Control:

1. desligado por padrão (`enable_keyboard_rgb`); desligado, a interface
   proprietária **não é tocada** (nem leituras);
2. só dispositivos USB ITE não-removíveis, com o descritor esperado e com id
   na lista verificada (`048d:c195`; outros ids aparecem como "detectado");
3. o efeito original do perfil é salvo em
   `/var/lib/legion-control/keyboard-profile-N.hex` **antes** da primeira
   sobrescrita (se não der para salvar, nada é gravado) e
   `legionctl keyboard restore` o devolve;
4. nada é gravado quando o teclado já mostra a cor pedida (a gravação é em
   flash); sem animações nem reaplicação periódica.

Ainda não validado: persistência após reboot/suspensão (pelo desenho do
protocolo deve persistir), interação com `Fn+Espaço`, e outros ids ITE
(`c197` etc.) — rode `legionctl hardware-scan` e abra uma issue.

### O que ainda precisa de validação no hardware real

As escritas foram testadas contra reproduções da árvore `/sys` deste modelo
e seguem a documentação do kernel, mas precisam ser confirmadas na máquina
(por isso ficam desligadas por padrão):

1. `fanN_target`: o firmware respeita o alvo em todos os perfis ou só em
   `custom`? Por quanto tempo antes de reescrever? (O daemon reaplica o alvo
   quando o valor lido difere.)
2. Limites `ppt_*`: confirmar efeito com `legionctl cpu status` (RAPL) em
   carga, no perfil `custom`.
3. O RAPL `long_term` (120 W) difere de `ppt_pl1_spl` (105 W) no perfil
   `performance` — relação a investigar.

Roteiro sugerido (como root, com o daemon parado para observar direto):

```sh
legionctl profile custom
legionctl --json fan status
echo 3000 | sudo tee /sys/class/hwmon/hwmonN/fan1_target   # N = lenovo_wmi_other
watch -n1 cat /sys/class/hwmon/hwmonN/fan1_input
echo 0 | sudo tee /sys/class/hwmon/hwmonN/fan1_target       # volta ao automático
```

## Futuros modelos

Legion 5, Legion 7, Legion Pro, Legion Slim: **sem dados**. Rode
`legionctl hardware-scan` e `scripts/collect-debug.sh` e abra uma issue com
os arquivos (revise antes de compartilhar).
