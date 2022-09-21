use std::{future::Future, pin::Pin, fmt::Debug, time::Duration};

use futures::future::select_all;
use serde::Deserialize;
use serde_json::de;
use tokio::{fs::File, io::AsyncWriteExt, time::sleep};

use crate::{TxData, Error, ARWEAVE_NODE};


const MAX_CHUNK_USIZE: usize = 256*1024;
const MAX_CONCURRENT_CONN: usize = 32;
const RETRY_COUNT: u32 = 3;
const RETRY_DELAY: Duration = Duration::new(10, 0);

///Download a file.
pub async fn download(data: TxData, outfile: impl Into<String>) -> Result<(), Error> {
    let mut outfile = File::create(outfile.into()).await.map_err(|e| Error::FileError(e.to_string()))?; //Open output file.
    let (end_chunks, end_size) = download_end_chunks(data).await?;
    let new_offset = data.offset - end_size as u128;
    let new_size = data.size - end_size;
    if new_size % MAX_CHUNK_USIZE != 0 {
        return Err(Error::ChunkSizingWeird);
    }
    println!("Recieved end chunks. Downloading concurrently.");
    download_rest_chunks(new_offset, new_size, &mut outfile).await?;

    //Write final chunks.
    for chunk in end_chunks.iter().rev() { //Reverse end chunks.
        println!("Writing {} bytes to file. (End Chunks)", chunk.len());
        outfile.write_all(chunk).await.map_err(|e| Error::FileError(e.to_string()))?; //Write end chunks.
    }
    println!("Finished writing bytes.");

    Ok(())
}

///Downloads the end chunks of the TX. Returns the data and the combined size of the chunks. DATA IS IN REVERSE ORDER.
async fn download_end_chunks(data: TxData) -> Result<(Vec<Vec<u8>>, usize), Error> {
    let mut found_data = 0;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    while found_data < data.size {
        let chunk = download_chunk_retry(data.offset - found_data as u128).await?;
        let length = chunk.len();
        chunks.push(chunk);
        found_data += length;
        if length == MAX_CHUNK_USIZE {
            return Ok((chunks, found_data));
        }
    }

    Ok((chunks, found_data))
}

async fn download_rest_chunks(end_offset: u128, remaining_size: usize, file_handle: &mut File) -> Result<(), Error> { //Download other chunks concurrently.
    let first_offset = end_offset - remaining_size as u128 + MAX_CHUNK_USIZE as u128; //Add chunk size because the offsets are at the END for some reason.
    let chunk_count = remaining_size / MAX_CHUNK_USIZE;
 
    //
    let mut completed_chunks = 0;
    let mut download_states: Vec<ChunkState> = (0..chunk_count).map(|_| ChunkState::Waiting).collect();
    while completed_chunks < chunk_count {
        //Iterate over offsets and chunk states.
        let mut write_sequential_broken = false;
        let mut pending_futs = Vec::new();
        for ((offset, index), chunk_state) in (completed_chunks..chunk_count).map(|i| ((i*MAX_CHUNK_USIZE) as u128 + first_offset, i)).zip(download_states.iter_mut().skip(completed_chunks)) {

            if !matches!(chunk_state, ChunkState::Ready(_)) {
                write_sequential_broken = true;
            }
            match chunk_state {
                ChunkState::Waiting if pending_futs.len() < MAX_CONCURRENT_CONN => { //Add more futures if the pending queue isn't full.
                    let fut = download_chunk_indexed(offset, index);
                    *chunk_state = ChunkState::Fetching(Box::pin(fut)); //Give ownership to the state array.
                    if let ChunkState::Fetching(inner_future) = &mut *chunk_state { //Get the value back out of the state.
                        pending_futs.push(inner_future);
                    } else { //What???
                        panic!("something impossible happened")
                    }
                },
                ChunkState::Ready(data) if !write_sequential_broken => { //Putting this below Fetching(...) causes the borrow checker to complain wtf...
                    println!("Writing {} bytes to file.", data.len());
                    file_handle.write_all(data).await.map_err(|err| Error::FileError(err.to_string()))?;
                    *chunk_state = ChunkState::WrittenToFile;
                    completed_chunks += 1; //Move processing start index by 1.
                },
                ChunkState::Fetching(fut) if pending_futs.len() < MAX_CHUNK_USIZE => {
                    pending_futs.push(fut); //Push pending future to queue.
                },
                ChunkState::WrittenToFile => panic!("Written to file should not be here. States: {download_states:?} Index: {index}."), //processing_start should be set past the written chunks.
                _ => ()
            }
        }

        if pending_futs.is_empty() { //Reiterate if there are no futures to be processed. This should only happen once the queue is finished.
            continue; 
        }

        let (data, index) = select_all(pending_futs).await.0?; //Get resolved request.
        let state = download_states.get_mut(index).unwrap(); //Get the state of the completed future.
        if !matches!(state, ChunkState::Fetching(_)) { //Safety check to check that the state is correct.
            panic!("A chunk state error occurred: States: {download_states:?} Index: {index}.");
        }

        *state = ChunkState::Ready(data); //Save data in state machine.
    }

    Ok(())
}

///Download a chunk at the specified offset. Returns the chunk id given so that it can be identified among other futures.
async fn download_chunk(offset: u128) -> Result<Vec<u8>, Error> {
    let url = format!("{ARWEAVE_NODE}/chunk/{}",offset.to_string());
    let response = reqwest::get(&url).await.map_err(|_|
        Error::RequestFailure
    )?;
    let code = response.status().as_u16();
    if code != 200 {
        return Err(Error::UnknownStatusCode(code));
    }
    let response_text = response.text().await.map_err(|e| {
        eprintln!(".text has failed!!!");
        Error::ArweaveBadResponse(e.to_string())
    })?;
    
    let chunk: ArweaveChunk = de::from_str(&response_text).map_err(|_| {
        eprintln!("Got weird response.");
        Error::ArweaveBadResponse("Arweave returned invalid JSON or the wrong data.".to_string())
    })?;
    //Convert chunk from base64url.
    let decoded_chunk = base64::decode_config(chunk.chunk, base64::URL_SAFE_NO_PAD).map_err(|_| Error::ArweaveBadResponse("Arweave returned invalid Base64.".to_string()))?;
    println!("Recieved data from: {}, Data Size: {}", url, decoded_chunk.len());
    Ok(decoded_chunk)
}

//Attempts to retry the download if it fails.
async fn download_chunk_retry(offset: u128) -> Result<Vec<u8>, Error> {
    let mut attempts = 0;
    loop {
        let dl = download_chunk(offset).await;
        match dl {
            Ok(data) => {
                return Ok(data); //Success
            },
            Err(e) => {
                attempts += 1; //Increment attempt counter.
                if attempts < RETRY_COUNT {
                    println!("Failed to download chunk on attempt #{}. Waiting {} seconds before retrying. Reason: {:?}", attempts, RETRY_DELAY.as_secs(), e);
                    sleep(RETRY_DELAY).await;
                } else {
                    return Err(e);
                }
            },
        }
    }
}

///Downloads a chunk and has an attached index to it.  
async fn download_chunk_indexed(offset: u128, index: usize) -> Result<(Vec<u8>, usize), Error> {
    Ok((download_chunk_retry(offset).await?, index))
}


enum ChunkState<'a> {
    Waiting,
    Fetching(Pin<Box<dyn Future<Output = Result<(Vec<u8>, usize), Error>> + 'a>>), //dumb inferred lifetime
    Ready(Vec<u8>),
    WrittenToFile
}

impl Debug for ChunkState<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Waiting => write!(f, "Waiting"),
            Self::Fetching(_) => write!(f, "Fetching"),
            Self::Ready(dat) => write!(f, "Ready {} bytes.", dat.len()),
            Self::WrittenToFile => write!(f, "WrittenToFile"),
        }
    }
}

#[derive(Deserialize)]
struct ArweaveChunk<'a> {
        chunk: &'a str
}