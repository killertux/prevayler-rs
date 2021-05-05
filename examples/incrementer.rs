use prevayler_rs::{
    error::PrevaylerResult, serializer::JsonSerializer, Prevayler, PrevaylerBuilder, Transaction,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Increment {
    increment: u8,
}

impl Transaction<u8> for Increment {
    fn execute(self, data: &mut u8) {
        *data += self.increment;
    }
}

#[async_std::main]
async fn main() -> PrevaylerResult<()> {
    let mut prevayler: Prevayler<Increment, _, _> = PrevaylerBuilder::new()
        .path(".")
        .serializer(JsonSerializer::new())
        .data(0 as u8)
        .build()
        .await?;
    prevayler
        .execute_transaction(Increment { increment: 1 })
        .await?;
    println!("{}", prevayler.query());
    Ok(())
}
