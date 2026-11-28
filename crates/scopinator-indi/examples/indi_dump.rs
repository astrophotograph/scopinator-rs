//! Read a capture of what an INDI server sent and list what is in it:
//! every device, its properties with their kind, permission, state and
//! members, and anything that could not be read.
//!
//! ```text
//! cargo run -p scopinator-indi --example indi_dump -- capture.xml
//! ```

use scopinator_indi::protocol::message::IndiMessage;
use scopinator_indi::protocol::parse::StreamParser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: indi_dump <file>")?;
    let bytes = std::fs::read(&path)?;
    let mut parser = StreamParser::new();
    let (mut defined, mut updates, mut others, mut errors) = (0, 0, 0, 0);
    // In pieces, as a socket would hand it over.
    for chunk in bytes.chunks(1500) {
        for message in parser.feed(chunk) {
            match message {
                Ok(IndiMessage::Define(property)) => {
                    defined += 1;
                    let members: Vec<String> = property
                        .members
                        .iter()
                        .map(|member| format!("{}={:?}", member.name, member.value))
                        .collect();
                    println!(
                        "def {:?} {} . {} [{:?} {:?}] {}",
                        property.kind,
                        property.device,
                        property.name,
                        property.permission,
                        property.state,
                        members.join(" ")
                    );
                }
                Ok(IndiMessage::Update(update)) => {
                    updates += 1;
                    println!(
                        "set {:?} {} . {} state={:?} {:?}",
                        update.kind, update.device, update.name, update.state, update.members
                    );
                }
                Ok(other) => {
                    others += 1;
                    println!("{other:?}");
                }
                Err(e) => {
                    errors += 1;
                    println!("ERROR {e}");
                }
            }
        }
    }
    println!("{defined} definitions, {updates} updates, {others} others, {errors} errors");
    Ok(())
}
