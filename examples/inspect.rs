//! Run with `cargo run --example inspect`.
//! The application uses std for printing; zmime itself only uses core.
use zmime::{decode_encoded_words, Limits, Message, Part};

const MAIL: &[u8] = b"Subject: =?UTF-8?Q?Build_report?=\r\n\
Content-Type: multipart/mixed; boundary=report\r\n\r\n\
--report\r\nContent-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\r\nBuild passed.=0A\r\n\
--report\r\nContent-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=sample.bin\r\n\
Content-Transfer-Encoding: base64\r\n\r\nAAEC/w==\r\n--report--\r\n";

fn main() {
    let mut workspace = [Part::EMPTY; 8];
    let message = Message::parse(MAIL, &mut workspace, Limits::DEFAULT).unwrap();
    let mut unfolded = [0u8; 128];
    let mut decoded = [0u8; 128];

    let subject = message.root().headers().get(b"subject").unwrap();
    let len = subject.unfolded(&mut unfolded).unwrap();
    let len = decode_encoded_words(&unfolded[..len], &mut decoded).unwrap();
    println!("Subject:{}", core::str::from_utf8(&decoded[..len]).unwrap());

    for (index, part) in message.parts().iter().enumerate() {
        let media = part.content_type();
        println!(
            "Part {index}: parent={:?}, {}/{}, {} raw body bytes",
            part.parent(),
            core::str::from_utf8(media.top_level()).unwrap(),
            core::str::from_utf8(media.subtype()).unwrap(),
            part.raw_body().len(),
        );
    }

    println!("Text: {}", message.parts()[1].text(&mut decoded).unwrap());
    let attachment = message.attachments().next().unwrap();
    // Listing the tree above never decoded this attachment. Read it on demand.
    let len = attachment.decode_body(&mut decoded).unwrap();
    assert_eq!(&decoded[..len], &[0, 1, 2, 255]);
    println!("Attachment: {:?}", &decoded[..len]);
}
