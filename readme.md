# Prevayler-rs
 
 This is a simple implementation of a system prevalance proposed by Klaus Wuestefeld in Rust.
 Other examples are the [prevayler](https://prevayler.org/) for Java and [prevayler-clj](https://github.com/klauswuestefeld/prevayler-clj) for Clojure.
 
 The idea is to save in a redolog all modifications to the prevailed data. If the system restarts, it will re-apply all transactions from the redolog restoring the system state. The system may also write snapshots from time to time to speed-up the recover process.
 
 Here is an example of a program that creates a prevailed state using an u8, increments it, print the value to the screen and closes the program.
 
 ```rust
 use prevayler_rs::{
     error::PrevaylerResult,
     PrevaylerBuilder,
     Prevayler,
     serializer::JsonSerializer,
     Transaction
 };
 use serde::{Deserialize, Serialize};
 
 #[derive(Serialize, Deserialize)]
 struct Increment {
     increment: u8
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
       .build().await?;
     prevayler.execute_transaction(Increment{increment: 1}).await?;
     println!("{}", prevayler.query());
     Ok(())
 }
 ```
 
 In most cases, you probably will need more than one transcation. The way that we have to do this now is to use an Enum as a main transaction that will be saved into the redolog. All transactions executed will then be converted into it.

 For more examples, take a look at the project tests