`github-app-test-key.pem` is a throwaway 2048-bit RSA key (PKCS#1, the format
GitHub downloads) generated for tests only. It signs app JWTs that the local
fake GitHub server in `tests/integrations_integration.rs` verifies. It is not
registered with GitHub or used anywhere else.
