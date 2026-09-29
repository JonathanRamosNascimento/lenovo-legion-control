# Legion Control — Linux

Desenvolva um aplicativo nativo para Linux, escrito em Rust, para controlar e monitorar notebooks Lenovo Legion, começando com suporte prioritário ao:

- Lenovo Legion 5 15IAX10
- Intel Core Ultra 7 255HX
- NVIDIA GeForce RTX 5070 Laptop GPU
- CachyOS / Arch Linux
- GNOME

O objetivo é criar uma alternativa Linux ao Lenovo Vantage/Legion Toolkit, com arquitetura segura, modular e extensível.

## 1. Objetivo

Criar o aplicativo "Legion Control", composto por:

1. Daemon privilegiado
2. Biblioteca/core Rust
3. CLI
4. Interface gráfica GTK4/libadwaita
5. Integração com systemd
6. Integração com D-Bus
7. Monitoramento de hardware
8. Sistema de perfis de desempenho

A aplicação deve priorizar segurança.

Nunca escrever diretamente em registradores EC desconhecidos ou interfaces ACPI não documentadas sem antes identificar claramente sua finalidade.

O aplicativo deve detectar automaticamente quais funcionalidades são suportadas pelo hardware.

---

# 2. Arquitetura

Utilizar workspace Cargo:

legion-control/

├── Cargo.toml
├── README.md
├── LICENSE
├── rustfmt.toml
│
├── crates/
│   ├── legion-core/
│   ├── legion-hardware/
│   ├── legion-acpi/
│   ├── legion-cpu/
│   ├── legion-gpu/
│   ├── legion-fan/
│   ├── legion-battery/
│   ├── legion-thermal/
│   ├── legion-profiles/
│   ├── legion-daemon/
│   ├── legion-cli/
│   └── legion-gui/
│
├── systemd/
│   └── legion-control.service
│
├── udev/
│   └── 99-legion-control.rules
│
├── scripts/
│   ├── detect-hardware.sh
│   └── collect-debug.sh
│
└── docs/
    ├── architecture.md
    ├── hardware-support.md
    └── development.md

---

# 3. Tecnologias

Usar:

- Rust stable
- Tokio
- serde
- serde_json
- anyhow
- thiserror
- tracing
- tracing-subscriber
- clap
- zbus
- GTK4
- libadwaita
- NVML quando disponível
- systemd
- sysfs
- procfs
- D-Bus

Evitar dependências desnecessárias.

O código deve seguir:

- Rust 2024 edition
- clippy
- rustfmt
- testes unitários
- testes de integração quando possível
- documentação pública para APIs

---

# 4. Hardware abstraction layer

Criar uma abstração:

trait HardwareController

Com capacidades:

- get_capabilities()
- get_power_profile()
- set_power_profile()
- get_cpu_info()
- get_gpu_info()
- get_temperatures()
- get_fan_info()
- get_battery_info()
- get_supported_features()

Exemplo:

enum PowerProfile {
    Quiet,
    Balanced,
    Performance,
    Custom,
    Unknown,
}

Exemplo de capabilities:

struct HardwareCapabilities {
    power_profiles: bool,
    fan_control: bool,
    fan_curve: bool,
    battery_threshold: bool,
    gpu_mode: bool,
    cpu_power_limit: bool,
    gpu_power_limit: bool,
    rgb_control: bool,
}

Nunca assumir que uma funcionalidade existe.

---

# 5. Hardware detection

Criar um módulo responsável por identificar:

- fabricante
- modelo
- motherboard
- BIOS
- CPU
- GPU
- memória
- kernel
- distribuição Linux
- desktop environment

Ler informações de:

/sys/devices/virtual/dmi/id/

/proc/cpuinfo

/sys/class/dmi/id/

/sys/class/hwmon/

Não executar comandos shell quando uma API do kernel/sysfs estiver disponível.

Registrar algo como:

Manufacturer: Lenovo
Product: Legion 5 15IAX10
Board: LNVNB161216
CPU: Intel Core Ultra 7 255HX
GPU: NVIDIA RTX 5070 Laptop GPU
OS: CachyOS
Kernel: ...
Desktop: GNOME

---

# 6. ACPI / Lenovo

Criar crate:

legion-acpi

Responsável por detectar interfaces ACPI/WMI relacionadas à Lenovo.

Primeiro apenas fazer leitura.

NÃO modificar nada durante a fase de descoberta.

Criar ferramenta:

legionctl hardware-scan

Ela deve gerar:

hardware-report.json

Contendo:

- DMI
- BIOS
- ACPI devices
- WMI devices
- power profiles
- hwmon
- thermal zones
- battery
- GPU
- CPU
- possíveis interfaces Lenovo

Exemplo:

{
  "manufacturer": "LENOVO",
  "product": "Legion 5 15IAX10",
  "board": "LNVNB161216",
  "cpu": "Intel Core Ultra 7 255HX",
  "features": {
    "power_profile": true,
    "fan_control": false,
    "battery_threshold": true
  }
}

---

# 7. Power profiles

Implementar:

legionctl profile

legionctl profile quiet

legionctl profile balanced

legionctl profile performance

legionctl profile custom

Primeiro utilizar interfaces Linux existentes.

Prioridade:

1. power-profiles-daemon
2. platform profile
3. Lenovo ACPI
4. outras interfaces específicas

Não duplicar gerenciamento de energia caso o kernel já ofereça a funcionalidade.

Detectar:

/sys/firmware/acpi/platform_profile

Se existir:

/sys/firmware/acpi/platform_profile_choices

Ler os valores disponíveis.

Nunca assumir que:

quiet
balanced
performance

existem exatamente com esses nomes.

Mapear os valores reais.

---

# 8. CPU

Implementar monitoramento:

- utilização
- frequência
- temperatura
- package power quando disponível
- EPP
- governor
- turbo

Detectar:

/sys/devices/system/cpu/

Implementar:

legionctl cpu status

Retorno:

CPU:
  Model: Intel Core Ultra 7 255HX
  Usage: 23%
  Frequency: 4200 MHz
  Temperature: 54 C
  Governor: schedutil
  EPP: balance_performance

Não alterar CPU power limits inicialmente.

Adicionar suporte somente depois que a interface for identificada e validada.

---

# 9. NVIDIA GPU

Implementar monitoramento usando NVML quando disponível.

Mostrar:

- temperatura
- utilização
- memória usada
- memória total
- clock
- power draw
- power limit
- fan speed
- driver
- GPU mode quando detectável

Comando:

legionctl gpu status

Exemplo:

GPU:
  Model: NVIDIA GeForce RTX 5070 Laptop GPU
  Temperature: 61 C
  Utilization: 87%
  VRAM: 4096 / 8192 MB
  Power: 92 W
  Driver: xxx

Não modificar power limit inicialmente.

---

# 10. Fans

Criar:

legion-fan

Primeiramente apenas leitura.

Detectar:

/sys/class/hwmon/

Mostrar:

legionctl fan status

Exemplo:

CPU Fan: 3200 RPM
GPU Fan: 2900 RPM

Somente implementar controle manual se for possível determinar uma interface segura e documentada.

Não escrever diretamente no Embedded Controller.

Criar uma capability:

fan_control_supported

Se false:

"Manual fan control is not supported on this hardware."

---

# 11. Temperatura

Ler:

/sys/class/thermal/

e hwmon.

Detectar:

- CPU
- GPU
- motherboard
- SSD
- outras zonas

Comando:

legionctl thermal

Exemplo:

CPU: 65 C
GPU: 62 C
SSD: 42 C

---

# 12. Battery

Criar:

legion-battery

Ler:

/sys/class/power_supply/

Implementar:

legionctl battery

Mostrar:

Battery:
  Status: Charging
  Capacity: 73%
  Health: Good
  Cycle count: ...
  Power: ...

Se houver suporte para threshold:

legionctl battery threshold 80

Antes de modificar:

- detectar interface
- validar valor
- verificar permissões
- registrar alteração

Nunca modificar valores fora do intervalo permitido.

---

# 13. Perfil Custom

Criar estrutura:

struct CustomProfile {
    cpu_epp: Option<String>,
    cpu_governor: Option<String>,
    platform_profile: Option<String>,
    battery_threshold: Option<u8>,
    fan_mode: Option<FanMode>,
}

Salvar em:

~/.config/legion-control/profiles/

Exemplo:

performance.json

quiet.json

custom.json

Não armazenar configurações root dentro do HOME do usuário.

---

# 14. Daemon

Criar:

legion-control-daemon

Executar como system service.

Responsabilidades:

- acessar hardware
- validar comandos
- expor D-Bus
- aplicar perfis
- monitorar hardware

A GUI NÃO deve acessar /sys diretamente.

Arquitetura:

GUI
 |
 | D-Bus
 v
Daemon
 |
 +-- CPU
 +-- GPU
 +-- ACPI
 +-- Battery
 +-- Fans
 +-- Thermal

---

# 15. D-Bus

Criar serviço:

com.legion.Control

Interface:

com.legion.Control.Manager

Métodos:

GetHardwareInfo()

GetCapabilities()

GetProfile()

SetProfile(profile)

GetCpuInfo()

GetGpuInfo()

GetBatteryInfo()

GetThermalInfo()

GetFanInfo()

GetStatus()

Signals:

ProfileChanged

TemperatureChanged

BatteryChanged

HardwareChanged

---

# 16. CLI

Criar:

legionctl

Comandos:

legionctl status

legionctl profile

legionctl profile quiet

legionctl profile balanced

legionctl profile performance

legionctl cpu status

legionctl gpu status

legionctl fan status

legionctl battery

legionctl thermal

legionctl hardware

legionctl hardware-scan

legionctl daemon status

legionctl version

Adicionar:

legionctl --json

para saída automatizada.

---

# 17. GUI

Criar aplicação GTK4 + libadwaita.

Nome:

Legion Control

Interface principal:

Dashboard

Mostrar:

CPU
GPU
Temperatura
RAM
Fans
Battery
Power profile

Exemplo visual:

┌──────────────────────────────────────┐
│ Legion Control                       │
│                                      │
│ Profile                              │
│                                      │
│ Quiet    Balanced    Performance     │
│                         ●            │
│                                      │
│ CPU                    GPU           │
│ 62°C                   58°C          │
│ 38%                    72%           │
│ 4.1 GHz                1900 MHz      │
│                                      │
│ Power                                │
│ CPU       38 W                       │
│ GPU       76 W                       │
│                                      │
│ Battery               82%            │
│ ████████████████░░                   │
└──────────────────────────────────────┘

Usar widgets nativos GTK/libadwaita.

Interface responsiva.

---

# 18. System tray

Adicionar integração com tray quando possível.

Menu:

Legion Control

Quiet

Balanced

Performance

Open

Quit

Não depender exclusivamente da tray para funcionamento.

---

# 19. Autostart

Criar:

~/.config/autostart/legion-control.desktop

A GUI deve iniciar automaticamente.

O daemon deve ser iniciado pelo systemd.

---

# 20. Segurança

Esta é uma prioridade.

Nunca:

- executar comandos recebidos da GUI diretamente via shell
- aceitar caminhos arbitrários
- escrever diretamente em EC desconhecido
- executar scripts fornecidos pelo usuário
- alterar BIOS
- modificar firmware
- alterar registradores desconhecidos

Validar absolutamente todos os valores.

O daemon deve possuir o mínimo possível de privilégios.

Sempre preferir:

sysfs
D-Bus
kernel API
ACPI documentado
NVML

antes de mecanismos proprietários.

---

# 21. Logs

Utilizar tracing.

Níveis:

ERROR
WARN
INFO
DEBUG
TRACE

Comando:

RUST_LOG=legion_control=debug legion-control-daemon

Nunca registrar:

- senhas
- tokens
- dados pessoais
- conteúdo desnecessário

---

# 22. Diagnóstico

Criar:

legionctl doctor

Executar diagnóstico completo:

- modelo
- CPU
- GPU
- kernel
- BIOS
- ACPI
- power profile
- hwmon
- thermal
- battery
- permissions
- D-Bus
- systemd

Resultado:

PASS
WARN
FAIL
UNSUPPORTED

Exemplo:

Hardware detection       PASS
ACPI                     PASS
Power profile            PASS
Battery threshold        PASS
GPU monitoring           PASS
Fan monitoring           PASS
Fan control              UNSUPPORTED

---

# 23. Testes

Criar testes para:

- parsing DMI
- parsing sysfs
- capabilities
- profiles
- validation
- temperature
- battery
- JSON
- CLI

Criar mocks para hardware.

O core não deve depender diretamente do hardware físico para ser testado.

---

# 24. Fases de desenvolvimento

FASE 1

Somente diagnóstico.

Implementar:

- hardware detection
- CPU
- GPU
- RAM
- battery
- temperatures
- fans read-only
- power profile read-only

Não modificar absolutamente nada.

Objetivo:

legionctl doctor

funcionar corretamente.

---

FASE 2

Power profiles.

Implementar:

quiet
balanced
performance

Usando as APIs existentes do Linux/firmware.

---

FASE 3

Daemon + D-Bus.

Separar acesso privilegiado da aplicação.

---

FASE 4

Battery threshold.

Implementar somente se a interface for suportada.

---

FASE 5

GUI.

Dashboard + profiles + monitoring.

---

FASE 6

Fan control.

Somente após identificar precisamente a interface suportada pelo Legion 5 15IAX10.

---

FASE 7

CPU/GPU advanced controls.

Adicionar somente controles comprovadamente suportados.

---

FASE 8

Custom profiles.

Permitir salvar e restaurar configurações.

---

# 25. Compatibilidade

O aplicativo deve ser desenvolvido com suporte genérico para Legion, mas possuir uma matriz:

Lenovo Legion 5 15IAX10
  CPU: Ultra 7 255HX
  Board: LNVNB161216

Future:

Legion 5
Legion 7
Legion Pro
Legion Slim

Cada modelo deve possuir capabilities próprias.

Nunca assumir que dois modelos Legion possuem o mesmo EC.

---

# 26. Performance

O daemon deve consumir poucos recursos.

Objetivo:

CPU:
< 1% em idle

RAM:
< 50 MB para daemon

GUI:
< 150 MB em idle

Monitoramento:

CPU/GPU:
1 segundo

Temperatura:
1 segundo

Battery:
5-10 segundos

Hardware discovery:
somente no startup ou quando solicitado.

---

# 27. UX

Quando uma funcionalidade não existir:

Não esconder.

Mostrar:

"Not supported on this hardware."

Quando existir mas exigir privilégio:

"Administrator permission required."

Quando houver erro:

mostrar uma mensagem compreensível.

Nunca simplesmente falhar silenciosamente.

---

# 28. Instalação

Criar suporte para Arch/CachyOS.

Preferir inicialmente:

makepkg

e depois criar:

PKGBUILD

Pacotes:

legion-control
legion-control-git

Instalar:

sudo pacman -U legion-control-*.pkg.tar.zst

Criar serviço:

sudo systemctl enable --now legion-control.service

---

# 29. Documentação

README deve conter:

- objetivo
- hardware suportado
- instalação
- configuração
- comandos
- troubleshooting
- segurança
- limitações
- desenvolvimento

Criar:

docs/hardware-support.md

explicando exatamente quais funcionalidades são suportadas em cada modelo.

---

# 30. Regra fundamental

Não tentar reproduzir o Lenovo Vantage por força bruta.

Primeiro descobrir quais interfaces o firmware do Legion 5 15IAX10 disponibiliza.

Depois implementar uma abstração segura.

A aplicação deve sempre detectar capabilities antes de oferecer controles.

O resultado deve ser um software Linux robusto, modular e seguro, e não apenas um conjunto de scripts para escrever em /sys ou EC.

---

# 31. Primeiro milestone

O primeiro milestone deve produzir:

legionctl doctor

e:

legionctl hardware-scan

Sem alterar nenhuma configuração do notebook.

Ao terminar essa etapa, apresentar:

1. Hardware detectado
2. Interfaces ACPI encontradas
3. Interfaces Lenovo encontradas
4. Power profiles disponíveis
5. Interfaces de fan encontradas
6. Battery controls encontrados
7. GPU/NVML disponível
8. Temperaturas disponíveis
9. Funcionalidades potencialmente suportadas
10. Funcionalidades ainda desconhecidas

Somente depois dessa análise começar a implementar comandos que alteram o hardware.

# 32. Regra para o agente de programação

Não inventar suporte.

Não assumir que uma interface funciona.

Não copiar código de outros projetos sem verificar licença.

Quando encontrar uma implementação existente em LenovoLegionLinux, analisar a abordagem e a licença antes de reutilizar qualquer código.

Documentar a origem de qualquer comportamento específico do hardware.

Priorizar código seguro e reversível.

Começar agora pela FASE 1.

Não implementar GUI ainda.

Primeiro entregar:

- workspace Rust
- crates
- hardware detection
- hardware-scan
- doctor
- CPU detection
- GPU detection
- RAM detection
- battery detection
- thermal detection
- fan read-only detection
- power profile detection
- testes
- README

Depois compilar e executar os testes.

Ao finalizar, mostrar a estrutura do projeto, comandos para executar e um exemplo da saída de:

legionctl doctor

Não realizar nenhuma alteração de hardware durante essa primeira fase.