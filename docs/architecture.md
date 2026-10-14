# Arquitetura

```
  legion-control (GUI, GTK4/libadwaita)      legionctl (CLI)
   │  thread D-Bus (polling 1 s)               │ leituras: direto (sem privilégio)
   │  nunca lê /sys                            │ escritas: daemon, ou direto se root
   └──────────────┐                 ┌──────────┘
                  ▼   D-Bus (system bus) + polkit
        ┌──────────────────────────────────────────┐
        │ legion-control-daemon  com.legion.Control │
        │  Tokio + zbus (async)                     │
        │  └─ thread "legion-hw" (única dona do HW) │
        │       cache de Status, loop de monitor,   │
        │       curva de ventoinha + segurança      │
        └──────────────────┬───────────────────────┘
                           ▼
                 legion-hardware  (LinuxController, config, capabilities,
                                   doctor, hardware-scan, custom profiles)
   ┌────────┬────────┬────────┬───────────┬───────────┬──────────┬────────────┐
 legion-  legion-  legion-  legion-      legion-     legion-    legion-
  cpu      gpu      fan     battery      thermal      acpi      profiles (+ppd)
   └────────┴────────┴────────┴───────────┴───────────┴──────────┴────────────┘
                           ▼
                      legion-core   tipos, trait HardwareController, SysRoot,
                                    write_attr (única escrita), hwmon, matriz
   legion-dbus: contrato D-Bus (nomes, proxy, ações polkit, mapeamento de erros)
```

## Princípios

* **Uma única primitiva de escrita**: `legion_core::sysfs::write_attr`.
  Recusa caminhos fora de `/sys`, `..`, valores multi-linha; nunca cria
  arquivos; registra `old → new` no alvo de log `legion_audit`; relê o
  valor. `grep write_attr` lista todos os pontos que alteram hardware.
* **Validar antes de escrever**, contra o que o kernel anuncia: choices do
  platform_profile, listas de EPP/governor, `min/max/scalar_increment` dos
  firmware-attributes, faixa e divisor de RPM, `charge_types` disponíveis.
  Operações em lote (ex.: todas as ventoinhas) validam tudo antes da
  primeira escrita.
* **Somente interfaces documentadas**: lista explícita de atributos de
  `lenovo-wmi-other` e de chips hwmon validados para ventoinha. Blocos WMI
  desconhecidos nunca são usados; o EC nunca é acessado.
* **Recursos experimentais desligados por padrão** (`daemon.json`).
* **`SysRoot`**: todo acesso a `/sys` e `/proc` é relativo a uma raiz, `/` em
  produção e uma árvore falsa nos testes (`FakeSysfs`,
  `legion_hardware::fixtures::legion_5_15iax10`).
* **Sem shell**, NVML carregado dinamicamente, D-Bus via zbus.
* **Economia**: NVML só com a dGPU ativa; GPU a cada 2 s, bateria a cada
  10 s, power-profiles-daemon a cada 5 s; demais leituras 1 s.
* **Privacidade**: serial, UUID, MAC nunca lidos.

## Daemon

* `Hw`: fila de tarefas para a thread `legion-hw`. Métodos D-Bus assíncronos
  enviam uma closure e aguardam o resultado; todas as escritas ficam
  serializadas e nenhuma chamada bloqueante roda no runtime Tokio.
* `Worker::tick()` (1 s): amostra CPU (uso e RAPL por diferença entre
  ticks), GPU, bateria, perfil, temperaturas, ventoinhas; gera eventos →
  sinais `ProfileChanged`, `TemperatureChanged` (Δ ≥ 1 °C),
  `BatteryChanged`, `HardwareChanged`.
* Controle de ventoinha: modo `auto | fixed | curve`. A cada tick verifica
  segurança (temperatura CPU/GPU, sensor ausente, falhas de escrita) e
  reaplica o alvo se o firmware o alterou. Ao receber SIGTERM/SIGINT
  devolve o controle ao firmware (alvo 0).
* polkit: `CheckAuthorization` com sujeito `system-bus-name` do chamador,
  uma ação por categoria (`com.legion.Control.set-profile`, `.battery`,
  `.cpu`, `.fan`, `.power-limits`, `.keyboard`). `ApplyProfile` exige todas as ações que
  o perfil toca. Voltar a ventoinha para automático não exige autorização.

## Interface D-Bus

Serviço `com.legion.Control`, objeto `/com/legion/Control`, interface
`com.legion.Control.Manager`. Getters retornam JSON (tipos de
`legion_core::model`):

`GetDaemonInfo, GetHardwareInfo, GetCapabilities, GetProfile, GetCpuInfo,
GetGpuInfo, GetBatteryInfo, GetThermalInfo, GetFanInfo, GetStatus,
GetPowerLimits, CaptureProfile`

Setters: `SetProfile(s)`, `SetBatteryConservation(b)`, `SetChargeType(s)`,
`SetChargeThreshold(y)`, `SetFanMode(s json)`,
`SetFirmwareAttribute(s, x)`, `SetCpuEpp(s)`, `SetCpuGovernor(s)`,
`SetTurbo(b)`, `SetKeyboardColor(s)`, `SetKeyboardBrightness(y)`,
`RestoreKeyboard()`, `ApplyProfile(s json) → s`.

Sinais: `ProfileChanged(s)`, `TemperatureChanged(s)`, `BatteryChanged(s)`,
`HardwareChanged()`.

Erros: `org.freedesktop.DBus.Error.{NotSupported, InvalidArgs,
AccessDenied, Failed}` com mensagem legível.

## Teclado

`legion-keyboard` é a única parte que escreve em um dispositivo que **não** é
`/sys`: *feature reports* HID em `/dev/hidrawN`. As regras são as mesmas das
demais escritas, adaptadas:

* **detecção sem privilégio**: `/sys/class/hidraw/*/device/{uevent,report_descriptor}`
  e o atributo `removable` do dispositivo USB; só ITE (`048d`), não-removível,
  com o relatório `7` de 959 bytes na coleção `0xff89/0x07` e id verificado;
* **validar antes de enviar**: nenhum relatório é enviado se o descritor não
  bater (`spectrum::check_descriptor`); respostas curtas/estranhas são erro;
* **uma sessão por operação**: abre o nó, confere a compatibilidade, opera e
  fecha; o `hidapi` fica restrito ao transporte (o código próprio continua
  `unsafe`-free);
* **só o perfil ativo** é lido/gravado e o conteúdo anterior é entregue ao
  chamador *antes* da gravação (backup em `/var/lib/legion-control`);
* **desligado por padrão** (`enable_keyboard_rgb`): desligado, o dispositivo
  proprietário nem é aberto;
* testes e `--sysroot` usam `SimSpectrum` (controlador simulado) no lugar do
  `hidraw`.

## Crates

| Crate | Responsabilidade |
|---|---|
| `legion-core` | tipos, trait, `SysRoot`/`write_attr`, hwmon, matriz de modelos, mocks |
| `legion-hardware` | detecção, `LinuxController`, config, capabilities, doctor, relatório, perfis custom, fixtures |
| `legion-acpi` | ACPI/WMI, drivers Lenovo, firmware-attributes (+ escrita validada) |
| `legion-cpu` | monitoramento + governor/EPP/turbo |
| `legion-gpu` | PCI/DRM, modo gráfico, NVML |
| `legion-fan` | RPM, alvo de ventoinha, curva com histerese |
| `legion-battery` | bateria, conservação, tipo de carga, limite |
| `legion-thermal` | thermal zones + hwmon |
| `legion-profiles` | platform_profile, ppd, `CustomProfile`, armazenamento |
| `legion-keyboard` | cor/brilho do teclado: parser de descritor HID, protocolo Spectrum, descoberta via sysfs, transporte `hidraw` (via `hidapi`), simulador |
| `legion-dbus` | contrato D-Bus compartilhado |
| `legion-i18n` | traduções (en, pt-BR) aplicadas na apresentação (CLI/GUI) |
| `legion-daemon` | `legion-control-daemon` |
| `legion-cli` | `legionctl` |
| `legion-gui` | `legion-control` |
