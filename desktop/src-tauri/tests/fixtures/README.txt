SYNTHETIC TEST-ONLY JWT fixture for local transport tests.
Run node scripts/tools/prepare-test-fixtures.mjs before cargo test.
The ignored RSA private fixture is generated locally; never upload it.
The script writes a matching public JWKS. No production credentials are used.
Production trusts only Microsoft discovery keys, never this test fixture.
