mod downloader;
mod test;

use std::{env, convert::{TryFrom, TryInto}};
use serde_json::de;
use serde::{Deserialize};

use crate::downloader::download;

pub const ARWEAVE_NODE: &'static str = "https://arweave.net";

#[tokio::main]
async fn main() {
    let result = go().await;
    if let Err(err) = result {
        match err {
            Error::InvalidArguments => {
                eprintln!("Failed to recognise arguments!");
                eprintln!("Correct usage: --transaction <tx id> --output <output file name>");
            },
            Error::RequestFailure => eprintln!("Request failed. Please try again."),
            Error::NotFound => eprintln!("The requested transaction or chunk could not be found."),
            Error::NodeNotReady => eprintln!("The node has not yet processed this transaction."),
            Error::UnknownStatusCode(code) => eprintln!("Arweave returned an unrecognised status code: {}", code),
            Error::ArweaveBadResponse(msg) => eprintln!("Arweave returned an invalid response(this is Arweave's fault). Please retry. Msg: {}", msg),
            Error::SizeTooBig => eprintln!("The transaction is too big (max 4GB on 32-bit systems)."),
            Error::SizeIsZero => eprintln!("The transaction is empty."),
            Error::FileError(msg) => eprintln!("An error occurred while opening or writing to the file: {}", msg),
            Error::ChunkSizingWeird => eprintln!("Arweave returned a chunk that was incorrectly sized.")
        }
    }
}

async fn go() -> Result<(), Error> {
    let args = get_args().ok_or(Error::InvalidArguments)?;
    println!("Attempting to receive transaction data.");
    let transaction_data = get_tx_data(args.transaction).await?;
    println!("Got transaction data. Starting download.");
    download(transaction_data, args.output).await?;
    println!("Download complete");
    Ok(())
}

fn get_args() -> Option<Args> {
    let mut arguments = env::args().into_iter();
    let mut transaction: Option<String> = None;
    let mut output: Option<String> = None;
    while let Some(arg) = arguments.next() {
        if arg.starts_with("--") {
            match arg.as_str() {
                "--transaction" => {
                    transaction = arguments.next();
                },
                "--output" => {
                    output = arguments.next();
                },
                _ => {} //Skip to next.
            }
        } //Ignore unknown arguments for now.
    }

    if let Some((transaction, output)) = transaction.zip(output) {
        Some(Args {
            transaction,
            output
        })
    } else {
        None
    }
}

pub(crate) async fn get_tx_data(tx_id: impl Into<String>) -> Result<TxData, Error> {
    let tx_id = tx_id.into();
    let url = format!("{ARWEAVE_NODE}/tx/{}/offset/", urlencoding::encode(&tx_id));
    println!("URL: \"{url}\"");
    let response = reqwest::get(url).await;
    match response {
        Ok(res) => {
            let code = res.status().as_u16();
            match code {
                200 => {
                    let text = res.text().await.map_err(|e| Error::ArweaveBadResponse(e.to_string()))?; //Failed to get text.
                    println!("Text: {text}");
                    let data_str: TxDataStr = de::from_str(&text).map_err(|_| Error::ArweaveBadResponse("Arweave returned invalid JSON.".to_string()))?; //Invalid JSON.
                    let data = data_str.try_into().map_err(|_| Error::ArweaveBadResponse("Arweave returned incorrect JSON.".to_string()))?; //Numbers not valid.
                    Ok(data)
                },
                404 => {
                    Err(Error::NotFound)
                },
                503 => {
                    Err(Error::NodeNotReady)
                }
                _ => {
                    Err(Error::UnknownStatusCode(code))
                }
            }
        },
        Err(_err) => {
            Err(Error::RequestFailure)
        },
    }
}

#[derive(Deserialize)]
struct TxDataStr<'a> {
    offset: &'a str,
    size: &'a str
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct TxData {
    offset: u128, //End offset
    size: usize //Size
}

impl TryFrom<TxDataStr<'_>> for TxData {
    type Error = ();

    fn try_from(value: TxDataStr) -> Result<Self, Self::Error> {
        Ok(TxData {
            offset: value.offset.parse().map_err(|_| ())?,
            size: value.size.parse().map_err(|_| ())?
        })
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidArguments,
    RequestFailure,
    NotFound,
    NodeNotReady,
    UnknownStatusCode(u16),
    ArweaveBadResponse(String),
    SizeTooBig,
    SizeIsZero,
    FileError(String),
    ChunkSizingWeird
}

#[derive(Debug)]
pub struct Args {
    transaction: String,
    output: String
}
