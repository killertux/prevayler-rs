// use std::convert::Infallible;

// use prevayler_rs::Transaction;
// use serde::{Deserialize, Serialize};

// #[derive(Serialize, Deserialize)]
// struct Increment {
//     increment: u8,
// }

// impl Transaction<u8> for Increment {
//     type Output = u8;

//     type Error = Infallible;

//     #[must_use]
// fn execute<'life0,'async_trait>(self,target: &'life0 mut Target) ->  ::core::pin::Pin<Box<dyn ::core::future::Future<Output = Result<Self::Output,Self::Error> > + ::core::marker::Send+'async_trait> >where 'life0:'async_trait,Self:'async_trait {
//         todo!()
//     }
// }

// #[tokio::main]
// async fn main() -> PrevaylerResult<()> {
//     let mut prevayler: Prevayler<Increment, _, _> = PrevaylerBuilder::new()
//         .path(".")
//         .serializer(JsonSerializer::new())
//         .data(0 as u8)
//         .build()
//         .await?;
//     prevayler
//         .execute_transaction(Increment { increment: 1 })
//         .await?;
//     println!("{}", prevayler.query());
//     Ok(())
// }

fn main() {}
