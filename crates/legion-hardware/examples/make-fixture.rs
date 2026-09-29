//! Writes the Legion 5 15IAX10 fixture tree to disk (development aid):
//!
//! ```sh
//! cargo run -p legion-hardware --features testing --example make-fixture
//! legionctl --sysroot <printed dir> doctor
//! legion-control-daemon --session --no-polkit --sysroot <printed dir>
//! ```

fn main() {
    let dir = legion_hardware::fixtures::legion_5_15iax10().keep();
    println!("{}", dir.display());
}
