#[cfg(test)]
mod download { //Okay so this works half the time when arweave isn't being stinky.
    use tokio::{fs::{File, remove_file}, io::AsyncReadExt};
    use crate::{get_tx_data, ARWEAVE_NODE};

    
    const SMALL_TX: &'static str = "THFUpuzn5jyHWLP3flrISA5ntVaBgtyPpxxXehXYDmw";
    const SMALL_DOWNLOAD_LOCATION: &'static str = "test_dl_s.bin";
    const MEDIUM_TX: &'static str = "oUnJ1xjWzmXdwJJgxybev__xOZGwq-2HlJZPHQKFO1Q";
    const MEDIUM_DOWNLOAD_LOCATION: &'static str = "test_dl_m.bin";

    async fn test_download(tx: &str, loc: &str) {
        let data = get_tx_data(tx).await.unwrap();
        crate::download(data, loc).await.unwrap();
        let correct_bytes: Vec<u8> = reqwest::get(format!("{}/{}", ARWEAVE_NODE, tx)).await.unwrap().bytes().await.unwrap().into();

        let mut file = File::open(loc).await.unwrap();
        let mut bytes = vec![0u8; file.metadata().await.unwrap().len() as usize];
        file.read_exact(&mut bytes).await.unwrap();
        remove_file(loc).await.unwrap(); //Cleanup
        if correct_bytes != bytes { //assert_eq! causes it to spam output.
            panic!("Test failed. Bytes did not match. Length of correct data: {}, Length of test data: {}", correct_bytes.len(), bytes.len());
        }
    }

    #[tokio::test]
    async fn test_medium_download(){ //Test a download with multiple chunks.
        test_download(MEDIUM_TX, MEDIUM_DOWNLOAD_LOCATION).await;
    }

    #[tokio::test]
    async fn test_small_download() { //Test a download with only one chunk <256kib.
        test_download(SMALL_TX, SMALL_DOWNLOAD_LOCATION).await;
    }
}