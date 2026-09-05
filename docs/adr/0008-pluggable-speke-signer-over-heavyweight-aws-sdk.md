# Pluggable SpekeSigner and SigV4Credentials over heavyweight AWS SDK

**Status: Accepted**

`drmpack` provides lightweight native API Key and `SigV4Credentials` data structures, and defines an extensible `SpekeSigner` trait seam rather than pulling in the official AWS SDK (`aws-config`, `aws-sdk-kms`, `aws-sigv4`) as direct dependencies.

## Context

AWS SPEKE (Secure Packager and Encoder Key Exchange) v2.0 key servers often run behind Amazon API Gateway and require authentication:
1. **API Key authentication (`x-api-key`)**: Commonly used by commercial DRM vendors (such as Axinom, EZDRM, BuyDRM) hosting SPEKE endpoints behind API Gateway without requiring IAM credentials.
2. **AWS SigV4 IAM authentication (`execute-api:Invoke`)**: Required when invoking private API Gateway endpoints secured by AWS IAM policies or IAM roles.
3. **Dynamic payload signing**: Under the AWS SigV4 specification for POST requests, the request signature is cryptographically bound to the SHA-256 hash of the request body (`x-amz-content-sha256`) and the current UTC timestamp (`x-amz-date`).

## Decision

1. **Native Lightweight Support**: `SpekeAuth` directly supports HTTP Basic, Bearer, API Key (`x-api-key`), and `SigV4Credentials` (encapsulating `Authorization`, `x-amz-date`, optional `x-amz-security-token`, and optional `x-amz-content-sha256`) with secret redaction in `fmt::Debug`.
2. **Pluggable Dynamic Signer Seam (`SpekeSigner`)**: Define the `SpekeSigner` trait:
   ```rust
   pub trait SpekeSigner: Send + Sync {
       fn sign(&self, builder: reqwest::RequestBuilder, endpoint: &str, body: &str) -> reqwest::RequestBuilder;
   }
   ```
   with a blanket implementation for closures `F: Fn(reqwest::RequestBuilder, &str, &str) -> reqwest::RequestBuilder + Send + Sync`.
3. **Decouple AWS SDK**: Do not add AWS SDK crates (`aws-config`, `aws-sigv4`, `aws-sdk-kms`, `aws-credential-types`) as crate dependencies.

## Considered options

- **Direct AWS SDK dependencies**: Bundling `aws-config` and `aws-sigv4` directly in `drmpack`.
  - *Rejected*: Adds dozens of transitively compiled crates (+100 dependencies), increases compile time significantly (+30-60s), bloats the binary size for non-AWS consumers, and creates dependency version conflicts if the consuming `media-server` application uses a different version of the AWS SDK.
- **Static SigV4 headers only without dynamic signer**: Only providing static header strings.
  - *Rejected*: Because SPEKE v2 requests generate dynamic CPIX 2.3 XML payloads with unique transaction IDs and timestamps at runtime, static SigV4 signatures fail AWS API Gateway signature verification unless a dynamic signer computes the payload SHA-256 hash per request.
- **Cargo feature flag with optional AWS SDK**: Gating the AWS SDK behind a `speke-sigv4-aws-sdk` feature flag.
  - *Rejected for now*: Adding `SpekeSigner` achieves complete decoupling. Applications already holding AWS SDK credentials can plug them into `SpekeConfig::with_signer(...)` in a 5-line closure without forcing `drmpack` to maintain AWS SDK bindings.

## Consequences

- Core `drmpack` remains fast to compile, lightweight, and completely decoupled from AWS SDK release cycles.
- Consumers needing live IAM/STS SigV4 signing can effortlessly integrate their existing AWS SDK client or custom KMS signing middleware via `SpekeConfig::with_signer(...)`.
- `SpekeExchangeResponse` and `SpekeClient` remain pure wire-protocol abstractions adhering strictly to the `KeyProvider` domain model.
