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
| RGB teclado | — | nenhum LED class de teclado (controladores USB HID não são sondados) | — |

Drivers de kernel observados: `lenovo_wmi_gamezone`, `lenovo_wmi_other`,
`lenovo_wmi_capdata`, `lenovo_wmi_events`, `lenovo_wmi_hotkey_utilities`,
`ideapad_acpi`, `ideapad_wmi`. 29 blocos WMI sem driver são listados no
relatório sem interpretação e **nunca** são usados.

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
