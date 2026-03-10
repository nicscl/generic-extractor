# Test Documents

Place your test PDFs and images here for local extraction testing.

## Quick Start

```bash
# Install CLI dependencies
cd cli && npm install && cd ..

# Start the server (in another terminal)
make run

# Extract a document
cd cli && npm run extract -- run ../test-docs/your-document.pdf
```

## CLI Commands

```bash
# Extract with default config (legal_br)
npm run extract -- run myfile.pdf

# Extract with specific config
npm run extract -- run myfile.pdf -c financial_br

# List available configs
npm run extract -- configs

# List test documents
npm run extract -- list

# Check extraction status
npm run extract -- status ext_abc123
```

## Output

Results are saved to `test-docs/output/` as JSON files.
