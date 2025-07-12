use actix_web::{App, HttpServer, Responder, HttpResponse, post, get, web};
use actix_multipart::Multipart;
use actix_cors::Cors;
use futures_util::stream::TryStreamExt; // Required for Multipart
use serde::{Deserialize, Serialize};
use std::collections::HashMap; // Import HashMap
use std::sync::Mutex; // Import Mutex for shared mutable state
use dotenv::dotenv; // To load .env file
use reqwest::Client; // To make HTTP requests (for IPFS API)

// --- Shared State ---
// Struct to hold shared mutable state
// We store a vector of tuples (CID, Filename) for each wallet
struct AppState {
    reports: Mutex<HashMap<String, Vec<(String, String)>>>,
}

// --- Response Structs ---
#[derive(Serialize, Deserialize, Debug)] // Add Debug for easier printing
struct UploadResponse {
    success: bool,
    message: String,
    cid: Option<String>,
    file_name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)] // Add Debug and Clone
struct ReportEntry {
    cid: String,
    file_name: String,
}

#[derive(Serialize, Deserialize, Debug)] // Add Debug
struct GetReportsResponse {
    success: bool,
    reports: Vec<ReportEntry>, // Array of ReportEntry structs
    message: Option<String>, // Optional error message
}


// --- Handlers ---

#[post("/api/upload-report")]
async fn upload_report(
    app_state: web::Data<AppState>, // Access the shared state
    mut payload: Multipart
) -> impl Responder {
    let mut target_wallet_address: Option<String> = None;
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;

    // Iterate over multipart fields to extract data
    while let Some(field_result) = payload.try_next().await.transpose() {
        let mut field = match field_result {
            Ok(field) => field,
            Err(e) => {
                eprintln!("Error parsing field: {:?}", e); // Log the error
                return HttpResponse::BadRequest().json(UploadResponse {
                    success: false,
                    message: format!("Error parsing field: {:?}", e),
                    cid: None,
                    file_name: None,
                });
            }
        };

        let content_disposition = field.content_disposition();
        let field_name = content_disposition.get_name().unwrap_or("");

        match field_name {
            "targetWalletAddress" => {
                // Read the field data into a String
                if let Some(bytes) = field.try_next().await.unwrap() {
                    target_wallet_address = Some(String::from_utf8_lossy(&bytes).to_string());
                }
            }
            "file" => {
                // Capture the filename
                file_name = content_disposition.get_filename().map(|s| s.to_string());
                // Read the entire file content into a Vec<u8>
                let mut bytes = Vec::new();
                while let Some(chunk) = field.try_next().await.unwrap() {
                    bytes.extend_from_slice(&chunk);
                }
                file_data = Some(bytes);
            }
            _ => {
                // Ignore unknown fields, but log for debugging
                eprintln!("Ignoring unknown field: {}", field_name);
            }
        }
    }

    // Validate required fields
    let file_name_str = match file_name {
        Some(name) => name,
        None => {
            eprintln!("File name missing in upload request.");
            return HttpResponse::BadRequest().json(UploadResponse {
                success: false,
                message: "File name is missing.".to_string(),
                cid: None,
                file_name: None,
            });
        }
    };

    let file_data_vec = match file_data {
        Some(data) => data,
        None => {
            eprintln!("File data missing in upload request.");
            return HttpResponse::BadRequest().json(UploadResponse {
                success: false,
                message: "File data is missing.".to_string(),
                cid: None,
                file_name: Some(file_name_str), // Return filename if available
            });
        }
    };

    let target_wallet = match target_wallet_address {
        Some(wallet) => wallet,
        None => {
            eprintln!("Target wallet address missing in upload request.");
             return HttpResponse::BadRequest().json(UploadResponse {
                success: false,
                message: "Target wallet address is required.".to_string(),
                cid: None,
                file_name: Some(file_name_str), // Return filename if available
            });
        }
    };


    // --- IPFS Upload ---
    let client = Client::new();
    let form = reqwest::multipart::Form::new()
        .part("file", reqwest::multipart::Part::bytes(file_data_vec)
            .file_name(file_name_str.clone())); // Clone filename for this part

    let ipfs_url = "http://127.0.0.1:5001/api/v0/add";
    let res = client.post(ipfs_url)
        .multipart(form)
        .send()
        .await;

    let res_json: serde_json::Value = match res {
        Ok(r) => {
            if !r.status().is_success() {
                 let status = r.status();
                 let body = r.text().await.unwrap_or_else(|_| "N/A".to_string());
                 eprintln!("IPFS upload failed with status {}: {}", status, body);
                 return HttpResponse::InternalServerError().json(UploadResponse {
                    success: false,
                    message: format!("IPFS upload failed with status: {}", status),
                    cid: None,
                    file_name: Some(file_name_str),
                 });
            }
            r.json().await.unwrap_or_else(|e| {
                 eprintln!("Failed to parse IPFS response JSON: {:?}", e);
                 serde_json::Value::Null // Return Null value on parse error
            })
        },
        Err(e) => {
            eprintln!("Failed to send request to IPFS API: {:?}", e); // Log the error
            return HttpResponse::InternalServerError().json(UploadResponse {
                success: false,
                message: format!("Failed to upload to IPFS: {}", e),
                cid: None,
                file_name: Some(file_name_str), // Return filename if available
            });
        }
    };

    let cid = res_json["Hash"].as_str().unwrap_or("").to_string();

    if cid.is_empty() {
         eprintln!("CID not found in IPFS response: {:?}", res_json);
         return HttpResponse::InternalServerError().json(UploadResponse {
            success: false,
            message: "Failed to get CID from IPFS response.".to_string(),
            cid: None,
            file_name: Some(file_name_str),
         });
    }

    println!("Successfully uploaded to IPFS, CID: {}", cid);

    // --- Simulate Storing in Shared State (Temporary) ---
    let mut reports_map = app_state.reports.lock().unwrap();
    let user_reports = reports_map.entry(target_wallet.clone()).or_insert_with(Vec::new);
    user_reports.push((cid.clone(), file_name_str.clone())); // Store tuple (CID, filename)
    println!("Stored (CID, filename) in backend map for {}: ({}, {})", target_wallet, cid, file_name_str);


    // TODO: Implement interaction with Solana smart contract:
    // - Use `target_wallet` (as a String) and `cid` (as a String)
    // - You'll need to convert `target_wallet` to a Solana `Pubkey` (using `bs58`)
    // - Construct a transaction that calls an instruction on your smart contract
    // - This instruction should record the link between the user's Pubkey and the CID
    // - Sign and send the transaction using a secure keypair on the backend


    // Send a success response back to the frontend with the real CID and filename
    HttpResponse::Ok().json(UploadResponse {
        success: true,
        message: "File uploaded to IPFS and recorded (simulated)".to_string(),
        cid: Some(cid),
        file_name: Some(file_name_str),
    })
}


#[get("/api/get-reports/{walletAddress}")]
async fn get_reports(
    app_state: web::Data<AppState>, // Access the shared state
    path: web::Path<String>
) -> impl Responder {
    let wallet_address = path.into_inner();
    println!("Backend received request for reports for wallet: {}", wallet_address);

    // Retrieve reports from the shared state (simulation of fetching from blockchain)
    let reports_map = app_state.reports.lock().unwrap();
    let user_reports_tuples = reports_map.get(&wallet_address);

    match user_reports_tuples {
        Some(reports_tuples) => {
            println!("Found {} reports for wallet: {}", reports_tuples.len(), wallet_address);
            // Convert the vector of tuples to a vector of ReportEntry structs
            let reports_list: Vec<ReportEntry> = reports_tuples.iter()
                .map(|(cid, filename)| ReportEntry {
                    cid: cid.clone(),
                    file_name: filename.clone(),
                })
                .collect();

            HttpResponse::Ok().json(GetReportsResponse {
                success: true,
                reports: reports_list, // Return the list of ReportEntry structs
                message: None,
            })
        }
        None => {
            println!("No reports found for wallet: {}", wallet_address);
            HttpResponse::Ok().json(GetReportsResponse {
                success: true,
                reports: vec![], // Return empty if none found
                message: Some("No reports found for this wallet".to_string()),
            })
        }
    }
}


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok(); // Load .env file

    println!("Starting Rust backend server on http://127.0.0.1:3001");

    // Create and configure the shared state
    let app_state = web::Data::new(AppState {
        reports: Mutex::new(HashMap::new()),
    });

    HttpServer::new(move || { // Use 'move' to move app_state into the closure
        let cors = Cors::default()
            .allowed_origin("http://localhost:5173") // Allow requests from your frontend
            .allowed_methods(vec!["GET", "POST"])
            .allowed_headers(vec!["*"]) // Be more specific in production
            .max_age(3600);

        App::new()
            .app_data(app_state.clone()) // Register shared state
            .wrap(cors) // Add CORS middleware
            .service(upload_report) // Register the upload endpoint
            .service(get_reports)    // Register the get reports endpoint
    })
    .bind("127.0.0.1:3001")? // Bind to the correct address and port
    .run()
    .await
}