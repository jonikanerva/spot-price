//! Better Auth compatibility of passwords and ids, against the vectors that
//! the real Better Auth code wrote (`tests/golden/http/auth-vectors.json`).

use serde::Deserialize;
use spot_price_server::auth::password::{hash_with_salt, verify_blocking};
use spot_price_server::auth::{js_length, random};

const VECTORS: &str = include_str!("../golden/http/auth-vectors.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vectors {
    password_vectors: Vec<PasswordVector>,
    signup: Signup,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordVector {
    label: String,
    password: String,
    utf16_length: usize,
    hash: String,
    verify: Vec<Candidate>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    candidate: String,
    result: bool,
}

#[derive(Debug, Deserialize)]
struct Signup {
    request: SignupRequest,
    rows: SignupRows,
}

#[derive(Debug, Deserialize)]
struct SignupRequest {
    password: String,
}

#[derive(Debug, Deserialize)]
struct SignupRows {
    user: Vec<serde_json::Value>,
    account: Vec<serde_json::Value>,
}

fn vectors() -> Vectors {
    serde_json::from_str(VECTORS).unwrap()
}

#[test]
fn hashes_and_verifies_like_better_auth() {
    for vector in vectors().password_vectors {
        let (salt, _) = vector.hash.split_once(':').unwrap();
        assert_eq!(
            hash_with_salt(&vector.password, salt).unwrap(),
            vector.hash,
            "{}",
            vector.label
        );
        assert_eq!(
            js_length(&vector.password),
            vector.utf16_length,
            "{}",
            vector.label
        );
        for candidate in vector.verify {
            assert_eq!(
                verify_blocking(&vector.hash, &candidate.candidate).unwrap(),
                candidate.result,
                "{} / {:?}",
                vector.label,
                candidate.candidate
            );
        }
    }
}

#[test]
fn the_signup_rows_have_the_format_of_the_generated_ones() {
    let signup = vectors().signup;
    let account = &signup.rows.account[0];
    let stored = account["password"].as_str().unwrap();
    assert!(verify_blocking(stored, &signup.request.password).unwrap());
    let user_id = signup.rows.user[0]["id"].as_str().unwrap();
    let ours = random::better_auth_id().unwrap();
    for id in [user_id, account["id"].as_str().unwrap(), ours.as_str()] {
        assert_eq!(id.len(), random::ID_LENGTH);
        assert!(id.bytes().all(|byte| byte.is_ascii_alphanumeric()), "{id}");
    }
    assert_eq!(account["accountId"], signup.rows.user[0]["id"]);
    assert_eq!(account["providerId"], "credential");
}
