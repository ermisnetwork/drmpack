# Mandatory Tenant Endpoints for Axinom DRM

Axinom Mosaic DRM architecture isolates each tenant with dedicated subdomains for Key Services (`https://<tenant-id>.key-service-management.axprod.net/api/SpekeV2`) and DRM License Services (`https://<tenant-id>.drm-{widevine,fairplay,playready}-licensing.axprod.net/AcquireLicense`). Previous versions of drmpack provided static default URLs (such as `https://key-server-management.axprod.net/api/SpekeV2` and mock staging license endpoints) and implemented the `Default` trait for `AxinomLicenseConfig` and `LicenseProxy`.

We eliminate all hardcoded default Axinom URLs (`DEFAULT_AXINOM_*`) and remove `impl Default` from `AxinomLicenseConfig` and `LicenseProxy`. Tenant-specific endpoints are now mandatory constructor parameters and required environment variables in `from_env()`. Furthermore, all documentation and examples fail fast when required environment variables are absent, rejecting dummy fallback credentials and mock key IDs.

## Considered options

- **Global static default URLs with optional env overrides**: Fallbacks to non-existent global domains silently cause 401 Unauthorized, 404 Not Found, or DNS resolution failures during packaging and license proxying.
- **Synthesizing URLs from Tenant ID alone**: While some standard patterns exist (`https://{tenant}.drm-widevine-licensing.axprod.net`), enterprise tenants often use customized gateway domains, regional clusters, or proxy layers that do not conform to standard subdomains.
- **Strict, explicit tenant endpoints (Chosen)**: Guarantees full tenant isolation, surfaces configuration errors immediately during startup (fail-fast), and prevents accidental transmission of customer DRM challenges or entitlement tokens to wrong or deprecated endpoints.
