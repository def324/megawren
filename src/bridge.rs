#[cxx::bridge(namespace = "megawren")]
pub mod ffi {
    struct Entry {
        id: u64,
        name: String,
        size: i64,
        folder: bool,
        modified: i64,
        shared: bool,
        exported: bool,
        parent: u64,
        fingerprint: String,
    }
    struct Event {
        done: bool,
        code: i32,
        bytes: i64,
        total: i64,
        retry: i32,
    }
    struct Account {
        ready: bool,
        pending: bool,
        code: i32,
        used: i64,
        total: i64,
    }
    #[derive(Debug)]
    enum LinkState {
        None,
        Active,
        Expired,
        TakenDown,
        Unknown,
    }
    struct Metadata {
        id: u64,
        parent: u64,
        path: String,
        created: i64,
        access: i32,
        in_rubbish: bool,
        restore: u64,
        generation: u64,
        restricted: bool,
        favourite: bool,
        direct_link: LinkState,
    }
    struct FolderStats {
        pending: bool,
        ready: bool,
        code: i32,
        files: i64,
        folders: i64,
        bytes: i64,
        versions: i64,
        version_bytes: i64,
    }
    unsafe extern "C++" {
        include!("native/client.hpp");
        include!("native/cache.hpp");
        type CacheCipher;
        fn cache_key(session: &[u8]) -> Result<Vec<u8>>;
        fn cache_cipher(key: &[u8], header: &[u8]) -> Result<UniquePtr<CacheCipher>>;
        fn header(self: &CacheCipher) -> Vec<u8>;
        fn encrypt(
            self: Pin<&mut CacheCipher>,
            bytes: &[u8],
            output: &mut [u8],
            final_chunk: bool,
        ) -> Result<()>;
        fn decrypt(self: Pin<&mut CacheCipher>, bytes: &[u8], output: &mut [u8]) -> Result<bool>;
        type Client;
        fn client(base: &str) -> Result<UniquePtr<Client>>;
        fn login(self: Pin<&mut Client>, email: &str, password: &str, otp: &str) -> Result<()>;
        fn restore(self: Pin<&mut Client>, session: &str) -> Result<()>;
        fn fetch(self: Pin<&mut Client>) -> Result<()>;
        fn fetch_current(self: Pin<&mut Client>) -> Result<()>;
        fn needs_checkpoint(self: &Client) -> bool;
        fn event(self: Pin<&mut Client>) -> Result<Event>;
        fn refresh_account(self: Pin<&mut Client>) -> Result<()>;
        fn account(self: Pin<&mut Client>) -> Result<Account>;
        fn connection(self: Pin<&mut Client>) -> Result<i32>;
        fn session(self: Pin<&mut Client>) -> Result<String>;
        fn root(self: Pin<&mut Client>) -> Result<u64>;
        fn rubbish(self: Pin<&mut Client>) -> Result<u64>;
        fn generation(self: &Client) -> u64;
        fn upload_location(self: Pin<&mut Client>, id: u64) -> Result<String>;
        fn metadata(self: Pin<&mut Client>, id: u64) -> Result<Metadata>;
        fn start_stats(self: Pin<&mut Client>, id: u64) -> Result<()>;
        fn folder_stats(self: Pin<&mut Client>) -> Result<FolderStats>;
        fn folders(
            self: Pin<&mut Client>,
            id: u64,
            offset: usize,
            limit: usize,
        ) -> Result<Vec<Entry>>;
        fn change(
            self: Pin<&mut Client>,
            kind: u8,
            source: u64,
            parent: u64,
            name: &str,
            generation: u64,
        ) -> Result<()>;
        fn utc_date(timestamp: i64) -> String;
        fn change_many(
            self: Pin<&mut Client>,
            kind: u8,
            sources: &[u64],
            parents: &[u64],
            names: &[String],
            generation: u64,
        ) -> Result<()>;
        fn info(self: Pin<&mut Client>, id: u64) -> Result<Entry>;
        fn children(
            self: Pin<&mut Client>,
            id: u64,
            offset: usize,
            limit: usize,
        ) -> Result<Vec<Entry>>;
        fn mkdir(self: Pin<&mut Client>, parent: u64, name: &str) -> Result<()>;
        fn result_node(self: Pin<&mut Client>) -> Result<u64>;
        fn public_file(self: Pin<&mut Client>, link: &str) -> Result<()>;
        fn public_folder(self: Pin<&mut Client>, link: &str) -> Result<()>;
        fn public_entry(self: Pin<&mut Client>) -> Result<Entry>;
        fn download(self: Pin<&mut Client>, id: u64, path: &str) -> Result<()>;
        fn named_children(
            self: Pin<&mut Client>,
            parent: u64,
            name: &str,
            offset: usize,
        ) -> Result<Vec<Entry>>;
        fn exists(self: Pin<&mut Client>, parent: u64, name: &str) -> Result<bool>;
        fn errors(self: Pin<&mut Client>) -> Result<String>;
        fn email(self: Pin<&mut Client>) -> Result<String>;
        fn preview(self: Pin<&mut Client>, id: u64, fd: i32, length: i64) -> Result<()>;
        fn preview_event(self: Pin<&mut Client>) -> Result<Event>;
        fn cancel_preview(self: Pin<&mut Client>) -> Result<()>;
        fn upload(
            self: Pin<&mut Client>,
            path: &str,
            parent: u64,
            name: &str,
            expected: &str,
        ) -> Result<()>;
        fn cancel(self: Pin<&mut Client>, revoke_session: bool) -> Result<()>;
        fn logout(self: Pin<&mut Client>) -> Result<()>;
    }
}
