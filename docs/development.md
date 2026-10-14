# Desenvolvimento

## Requisitos

* Rust stable ≥ 1.85 (edition 2024)
* `gtk4` ≥ 4.12 e `libadwaita` ≥ 1.7 (GUI)
* Opcional: `nvidia-utils` (NVML, carregado em tempo de execução),
  `dbus-daemon` (teste de integração do daemon)

## Comandos

```sh
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
cargo build --release -p legion-cli --no-default-features   # sem NVML
cargo build --release -p legion-gui --no-default-features   # sem bandeja
```

## Testes sem hardware (e sem root)

* Parsers e escritas recebem um `SysRoot`; os testes usam `FakeSysfs`.
* `legion_hardware::fixtures::legion_5_15iax10()` reproduz o Legion 5
  15IAX10. Para gravá-la em disco:
  `cargo run -p legion-hardware --features testing --example make-fixture`.
* `legionctl --sysroot DIR ...` (opção oculta) lê **e escreve** na árvore
  falsa, sem D-Bus/NVML — usado em `crates/legion-cli/tests/cli.rs`.
* `crates/legion-daemon/tests/dbus.rs` sobe um `dbus-daemon` privado, o
  daemon real sobre a fixture e exercita todo o contrato D-Bus (perfis,
  sinais, bateria, ventoinhas, limites, perfis custom, restauração ao
  SIGTERM).

## Rodar o daemon e a GUI sem root

```sh
FIX=$(cargo run -q -p legion-hardware --features testing --example make-fixture)
dbus-run-session -- sh -c "
  target/debug/legion-control-daemon --session --no-polkit --sysroot $FIX &
  target/debug/legion-control --session"
```

`--session`, `--no-polkit` (só com `--session`) e `--sysroot` existem
apenas para desenvolvimento; no barramento de sistema o daemon exige root
e usa polkit.

A GUI também aceita `--page <dashboard|battery|fans|performance|custom|keyboard|profiles|hardware>`.
Para capturas sem tocar na área de trabalho, use o backend Broadway:
`gtk4-broadwayd --port 8087 :7` e
`GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 GDK_DEBUG=no-portals legion-control --session`.

## Teclado em hardware real (sem daemon)

`legion-keyboard` traz um controle manual que usa exatamente o mesmo código do
daemon, útil para validar um modelo novo (como root):

```sh
cargo build -p legion-keyboard --example spectrum-ctl
sudo target/debug/examples/spectrum-ctl status            # só leitura
sudo target/debug/examples/spectrum-ctl color '#ff0000'   # cria keyboard-backup.hex na pasta atual
sudo target/debug/examples/spectrum-ctl restore
```

Cuidado: nunca peça o efeito de um perfil que não seja o ativo — isso também
o ativa (veja `docs/hardware-support.md`).

## Logs

```sh
RUST_LOG=debug legionctl doctor
RUST_LOG=legion_audit=info,legion_control_daemon=debug legion-control-daemon --session ...
journalctl -u legion-control -f
```

## Regras para novas funcionalidades

1. Descobrir a interface (`hardware-scan`) e documentar a origem em
   `docs/hardware-support.md`.
2. Implementar a leitura com testes em `FakeSysfs`.
3. Escrita: validar contra o que o kernel anuncia, usar apenas
   `sysfs::write_attr`, expor pelo daemon com uma ação polkit, adicionar
   teste de fixture e de D-Bus.
4. Se a semântica não estiver documentada, manter atrás de uma flag em
   `daemon.json` e marcar como experimental.
5. Não copiar código de outros projetos (ex.: LenovoLegionLinux, GPL-2.0)
   sem checar licença; este projeto é MIT.
