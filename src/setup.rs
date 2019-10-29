//! Utilities used during the initial setup

use crate::Pool;
use actix_web::middleware::Logger;
use blake2::{Blake2b, Digest};
use diesel::{
    r2d2::{self, ConnectionManager},
    sqlite::SqliteConnection,
};
use std::{env, path::PathBuf};

#[cfg(not(feature = "dev"))]
use dirs;
#[cfg(feature = "dev")]
use dotenv;
#[cfg(feature = "dev")]
use std::str::FromStr;
#[cfg(not(feature = "dev"))]
use std::{
    fs,
    io::{self, BufRead},
    process,
};
#[cfg(not(feature = "dev"))]
use toml;

/// Returns a path to the directory storing application data
#[cfg(not(feature = "dev"))]
pub fn get_data_dir() -> PathBuf {
    let base_dir = dirs::data_dir().expect("Unable to determine the data directory");
    base_dir.join(env!("CARGO_PKG_NAME"))
}

/// Returns a path to the directory storing application config
#[cfg(not(feature = "dev"))]
pub fn get_config_dir() -> PathBuf {
    let base_dir = dirs::config_dir().expect("Unable to determine the config directory");
    base_dir.join(env!("CARGO_PKG_NAME"))
}

/// Returns a path to the configuration file
#[cfg(not(feature = "dev"))]
fn get_config_path() -> PathBuf {
    get_config_dir().join("config.toml")
}

/// Returns a path to the bearer token hash
#[cfg(not(feature = "dev"))]
pub fn get_password_path() -> PathBuf {
    get_config_dir().join("passwd")
}

/// Returns the BLAKE2b digest of the input string
pub fn hash<T: AsRef<[u8]>>(input: T) -> Vec<u8> {
    let mut hasher = Blake2b::new();
    hasher.input(input);
    hasher.result().to_vec()
}

/// Returns an environment variable and panic if it isn't found
#[cfg(feature = "dev")]
#[macro_export]
macro_rules! get_env {
    ($k:literal) => {
        std::env::var($k).expect(&format!("Can't find {} environment variable", $k));
    };
}

/// Returns a parsed environment variable and panic if it isn't found or is not parsable
#[cfg(feature = "dev")]
macro_rules! parse_env {
    ($k:literal) => {
        get_env!($k).parse().expect(&format!("Invalid {}", $k))
    };
}

/// Application configuration
#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(not(feature = "dev"), serde(default))]
pub struct Config {
    /// Port to listen on
    pub port: u16,
    /// SQLite database connection url
    pub database_url: String,
    /// SQLite database connection pool size
    pub pool_size: u32,
    /// Directory where to store static files
    pub files_dir: PathBuf,
    /// Maximum allowed file size
    pub max_filesize: usize,
    /// SSL Certificate private key location
    pub cert_privkey: String,
    /// SSL Certificate chain location
    pub cert_chain: String,
    /// Use SSL or not
    pub use_ssl: bool,
}

#[cfg(not(feature = "dev"))]
impl Default for Config {
    fn default() -> Self {
        let port = 8080;
        let database_url = {
            let path = get_data_dir().join("database.db");
            path.to_str()
                .expect("Can't convert database path to string")
                .to_owned()
        };
        let pool_size = std::cmp::max(1, num_cpus::get() as u32 / 2);
        let files_dir = get_data_dir().join("files");
        let max_filesize = 10_000_000;
        let cert_privkey = "cert.pem".to_string();
        let cert_chain = "chain.pem".to_string();
        let use_ssl = false;

        Config {
            port,
            database_url,
            pool_size,
            files_dir,
            max_filesize,
            cert_privkey,
            cert_chain,
            use_ssl,
        }
    }
}

impl Config {
    /// Deserialize the config file
    #[cfg(not(feature = "dev"))]
    pub fn read_file() -> Result<Self, &'static str> {
        let path = get_config_path();
        let contents = if let Ok(contents) = fs::read_to_string(&path) {
            contents
        } else {
            return Err("Can't read config file.");
        };
        let result = toml::from_str(&contents);

        if result.is_err() {
            return Err("Invalid config file.");
        }
        let mut result: Config = result.unwrap();

        if result.files_dir.is_absolute() {
            if fs::create_dir_all(&result.files_dir).is_err() {
                return Err("Can't create files_dir.");
            }

            result.files_dir = match result.files_dir.canonicalize() {
                Ok(path) => path,
                Err(_) => return Err("Invalid files_dir."),
            }
        } else {
            let files_dir = get_data_dir().join(&result.files_dir);

            if fs::create_dir_all(&files_dir).is_err() {
                return Err("Can't create files_dir.");
            }

            result.files_dir = match files_dir.canonicalize() {
                Ok(path) => path,
                Err(_) => return Err("Invalid files_dir."),
            }
        }

        Ok(result)
    }

    /// Serialize the config file
    #[cfg(not(feature = "dev"))]
    pub fn write_file(&self) -> Result<(), &'static str> {
        let path = get_config_path();
        let contents = toml::to_string(&self).expect("Can't serialize config.");
        match fs::write(&path, &contents) {
            Ok(_) => Ok(()),
            Err(_) => Err("Can't write config file."),
        }
    }

    /// Creates a config from environment variables
    #[cfg(feature = "dev")]
    pub fn debug() -> Self {
        dotenv::dotenv().ok();

        let port = parse_env!("PORT");
        let database_url = get_env!("DATABASE_URL");
        let pool_size = parse_env!("POOL_SIZE");
        let files_dir = {
            let files_dir = get_env!("FILES_DIR");
            let path = PathBuf::from_str(&files_dir).expect("Can't convert files dir to path");
            if path.is_absolute() {
                path.canonicalize().expect("Invalid FILES_DIR")
            } else {
                let cargo_manifest_dir = env!("CARGO_MANIFEST_DIR");
                let mut cargo_manifest_dir = PathBuf::from_str(cargo_manifest_dir)
                    .expect("Can't convert cargo manifest dir to path");
                cargo_manifest_dir.push(&path);
                cargo_manifest_dir
                    .canonicalize()
                    .expect("Invalid FILES_DIR")
            }
        };
        let max_filesize = parse_env!("MAX_FILESIZE");
        let cert_privkey = parse_env!("CERTIFICATE");
        let cert_chain = parse_env!("CERT_CHAIN");
        let use_ssl = parse_env!("USE_SSL");

        Config {
            port,
            database_url,
            pool_size,
            files_dir,
            max_filesize,
            cert_privkey,
            cert_chain,
            use_ssl,
        }
    }
}

/// Creates a SQLite database connection pool
pub fn create_pool(url: &str, size: u32) -> Pool {
    let manager = ConnectionManager::<SqliteConnection>::new(url);
    r2d2::Pool::builder()
        .max_size(size)
        .build(manager)
        .expect("Can't create pool")
}

/// Initializes the logger
pub fn init_logger() {
    if cfg!(feature = "dev") && env::var_os("RUST_LOG").is_none() {
        env::set_var("RUST_LOG", "actix_web=debug");
    } else if !cfg!(feature = "dev") {
        env::set_var("RUST_LOG", "actix_web=info");
    }
    env_logger::init();
}

/// Returns the logger middleware
pub fn logger_middleware() -> Logger {
    #[cfg(feature = "dev")]
    {
        dotenv::dotenv().ok();
        if let Ok(format) = env::var("LOG_FORMAT") {
            Logger::new(&format)
        } else {
            Logger::default()
        }
    }

    #[cfg(not(feature = "dev"))]
    {
        Logger::default()
    }
}

/// Performs the initial setup
#[cfg(not(feature = "dev"))]
pub fn init() -> Config {
    fs::create_dir_all(get_config_dir()).unwrap_or_else(|e| {
        eprintln!("Can't create config directory: {}.", e);
        process::exit(1);
    });

    let password_path = get_password_path();
    if !password_path.exists() {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut password = String::new();

        loop {
            println!("Enter the password to use: ");
            stdin.read_line(&mut password).unwrap_or_else(|e| {
                eprintln!("Can't read password: {}", e);
                process::exit(1);
            });

            password = password.replace("\r", "");
            password = password.replace("\n", "");
            if !password.is_empty() {
                break;
            }

            println!("Are you sure you want to leave an empty password? This will disable authentication: [y/N]: ");
            let mut answer = String::new();
            stdin.read_line(&mut answer).unwrap_or_else(|e| {
                eprintln!("Can't read answer: {}", e);
                process::exit(1);
            });

            if answer.trim() == "y" {
                break;
            }
        }

        let password_hash = hash(&password);
        fs::write(&password_path, password_hash.as_slice()).unwrap_or_else(|e| {
            eprintln!("Can't write password: {}", e);
            process::exit(1);
        });
    }

    let config_path = get_config_path();
    if !config_path.exists() {
        println!("Generating config file at {}", config_path.display());
        let config = Config::default();
        config.write_file().unwrap_or_else(|e| {
            eprintln!("Can't write config file: {}", e);
            process::exit(1);
        });
        return config;
    }

    Config::read_file().unwrap_or_else(|e| {
        eprintln!("{}", e);
        process::exit(1);
    })
}
