#!/usr/bin/env node
import { program } from "commander";
import chalk from "chalk";
import ora from "ora";
import fs from "fs";
import path from "path";
import FormData from "form-data";
import fetch from "node-fetch";

const API_URL = process.env.EXTRACTOR_API_URL || "http://localhost:3002";
const TEST_DOCS_DIR = path.join(import.meta.dirname, "..", "test-docs");
const OUTPUT_DIR = path.join(TEST_DOCS_DIR, "output");

program
  .name("extract")
  .description("Local document extraction CLI")
  .version("1.0.0");

program
  .command("run <file>")
  .description("Extract structure from a document")
  .option("-c, --config <name>", "Extraction config to use", "legal_br")
  .option("--ocr <provider>", "OCR provider: gemini, docling, mistral_ocr", "gemini")
  .option("-o, --output <file>", "Output file path")
  .option("--include-raw", "Include raw OCR markdown in output")
  .option("--no-poll", "Submit only, don't wait for completion")
  .action(async (file, options) => {
    await runExtraction(file, options);
  });

program
  .command("status <id>")
  .description("Check extraction status")
  .action(async (id) => {
    await checkStatus(id);
  });

program
  .command("configs")
  .description("List available extraction configs")
  .action(async () => {
    await listConfigs();
  });

program
  .command("list")
  .description("List files in test-docs folder")
  .action(() => {
    listTestDocs();
  });

program.parse();

async function runExtraction(file, options) {
  // Resolve file path
  let filePath = file;
  if (!path.isAbsolute(file)) {
    // Check if it's in test-docs
    const testDocPath = path.join(TEST_DOCS_DIR, file);
    if (fs.existsSync(testDocPath)) {
      filePath = testDocPath;
    } else if (!fs.existsSync(file)) {
      console.error(chalk.red(`File not found: ${file}`));
      console.log(chalk.gray(`Tip: Place files in test-docs/ folder`));
      process.exit(1);
    }
  }

  if (!fs.existsSync(filePath)) {
    console.error(chalk.red(`File not found: ${filePath}`));
    process.exit(1);
  }

  console.log(chalk.blue("Document Extraction"));
  console.log(chalk.gray("─".repeat(40)));
  console.log(`File:   ${chalk.white(path.basename(filePath))}`);
  console.log(`Config: ${chalk.white(options.config)}`);
  console.log(`OCR:    ${chalk.white(options.ocr)}`);
  console.log(`API:    ${chalk.gray(API_URL)}`);
  console.log();

  // Submit extraction
  const spinner = ora("Submitting extraction...").start();

  try {
    const form = new FormData();
    form.append("file", fs.createReadStream(filePath));

    const ocrProvider = options.ocr === "gemini" ? "gemini_ocr" : options.ocr;
    const response = await fetch(
      `${API_URL}/extract?config=${options.config}&ocr_provider=${ocrProvider}`,
      {
        method: "POST",
        body: form,
      }
    );

    if (!response.ok) {
      const error = await response.text();
      spinner.fail("Submission failed");
      console.error(chalk.red(error));
      process.exit(1);
    }

    const result = await response.json();
    const id = result.id;

    spinner.succeed(`Submitted: ${chalk.cyan(id)}`);

    if (options.poll === false) {
      console.log(chalk.gray(`Check status with: npm run extract status ${id}`));
      return;
    }

    // Poll for completion
    await pollExtraction(id, filePath, options.output, options.includeRaw);
  } catch (err) {
    spinner.fail("Request failed");
    console.error(chalk.red(err.message));
    if (err.cause?.code === "ECONNREFUSED") {
      console.log(chalk.yellow("\nIs the server running? Try: make run"));
    }
    process.exit(1);
  }
}

async function pollExtraction(id, originalFile, outputPath, includeRaw) {
  const spinner = ora("Processing...").start();
  const startTime = Date.now();
  const maxWait = 5 * 60 * 1000; // 5 minutes

  while (Date.now() - startTime < maxWait) {
    try {
      const url = includeRaw
        ? `${API_URL}/extractions/${id}?include_raw=true`
        : `${API_URL}/extractions/${id}`;
      const response = await fetch(url);
      const result = await response.json();

      const elapsed = Math.round((Date.now() - startTime) / 1000);
      const step = result.current_step || "processing";

      switch (result.status) {
        case "completed":
          spinner.succeed(`Completed in ${elapsed}s`);
          await saveAndDisplayResult(result, originalFile, outputPath);
          return;

        case "failed":
          spinner.fail("Extraction failed");
          console.error(chalk.red(result.error || "Unknown error"));
          process.exit(1);
          break;

        case "processing":
          spinner.text = `Processing: ${step} (${elapsed}s)`;
          break;
      }
    } catch (err) {
      spinner.text = `Waiting... (${Math.round((Date.now() - startTime) / 1000)}s)`;
    }

    await sleep(2000);
  }

  spinner.fail("Timeout waiting for extraction");
  process.exit(1);
}

async function saveAndDisplayResult(result, originalFile, outputPath) {
  // Ensure output directory exists
  if (!fs.existsSync(OUTPUT_DIR)) {
    fs.mkdirSync(OUTPUT_DIR, { recursive: true });
  }

  // Generate output filename
  const basename = path.basename(originalFile, path.extname(originalFile));
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const outFile = outputPath || path.join(OUTPUT_DIR, `${basename}_${timestamp}.json`);

  // Save full result
  fs.writeFileSync(outFile, JSON.stringify(result, null, 2));
  console.log(chalk.green(`\nSaved: ${outFile}`));

  // Save raw markdown separately if present
  if (result.raw_markdown) {
    const rawFile = outFile.replace(/\.json$/, "_raw.md");
    fs.writeFileSync(rawFile, result.raw_markdown);
    console.log(chalk.green(`Raw OCR: ${rawFile} (${result.raw_markdown.length} chars)`));
  }

  // Display summary
  console.log(chalk.blue("\nSummary"));
  console.log(chalk.gray("─".repeat(40)));
  console.log(result.summary || "(no summary)");

  if (result.readable_id) {
    console.log(chalk.gray(`\nID: ${result.readable_id}`));
  }

  // Display structure
  if (result.children?.length > 0) {
    console.log(chalk.blue("\nStructure"));
    console.log(chalk.gray("─".repeat(40)));
    displayTree(result.children, 0);
  }

  // Display entities if present
  if (result.reference_index?.entities) {
    const entities = result.reference_index.entities;
    const entityTypes = Object.keys(entities);
    if (entityTypes.length > 0) {
      console.log(chalk.blue("\nEntities Found"));
      console.log(chalk.gray("─".repeat(40)));
      for (const type of entityTypes) {
        const values = entities[type].map((e) => e.value).slice(0, 5);
        console.log(`${chalk.cyan(type)}: ${values.join(", ")}${entities[type].length > 5 ? "..." : ""}`);
      }
    }
  }

  // Display OCR metadata if present
  if (result.ocr_metadata && Object.keys(result.ocr_metadata).length > 0) {
    const ocr = result.ocr_metadata;
    console.log(chalk.blue("\nOCR Info"));
    console.log(chalk.gray("─".repeat(40)));
    console.log(`${chalk.cyan("Provider")}: ${ocr.provider || "unknown"} (${ocr.model || "?"})`);
    if (ocr.token_usage) {
      const t = ocr.token_usage;
      console.log(`${chalk.cyan("Tokens")}: ${t.prompt_tokens?.toLocaleString() || 0} in / ${t.output_tokens?.toLocaleString() || 0} out`);
    }
    if (ocr.cost_usd !== undefined) {
      console.log(`${chalk.cyan("Cost")}: $${ocr.cost_usd.toFixed(4)}`);
    }
    if (ocr.finish_reason) {
      const reason = ocr.finish_reason;
      const color = reason === "STOP" ? chalk.green : chalk.yellow;
      console.log(`${chalk.cyan("Status")}: ${color(reason)}`);
    }
  }
}

function displayTree(nodes, depth) {
  const indent = "  ".repeat(depth);
  const connector = depth === 0 ? "" : "├─ ";

  for (const node of nodes) {
    const type = chalk.yellow(node.type);
    const subtype = node.subtype ? chalk.gray(` (${node.subtype})`) : "";
    const label = node.label || node.id;
    const pages = node.page_range ? chalk.gray(` [p${node.page_range[0]}-${node.page_range[1]}]`) : "";

    console.log(`${indent}${connector}${type}${subtype}: ${label}${pages}`);

    if (node.children?.length > 0) {
      displayTree(node.children, depth + 1);
    }
  }
}

async function checkStatus(id) {
  const spinner = ora("Fetching status...").start();

  try {
    const response = await fetch(`${API_URL}/extractions/${id}`);
    const result = await response.json();

    spinner.stop();

    console.log(chalk.blue(`Extraction: ${id}`));
    console.log(chalk.gray("─".repeat(40)));
    console.log(`Status: ${statusColor(result.status)}`);

    if (result.current_step) {
      console.log(`Step:   ${result.current_step}`);
    }
    if (result.error) {
      console.log(`Error:  ${chalk.red(result.error)}`);
    }
    if (result.status === "completed") {
      console.log(`Pages:  ${result.total_pages || "?"}`);
      console.log(`Nodes:  ${result.children?.length || 0}`);
    }
  } catch (err) {
    spinner.fail("Failed to fetch status");
    console.error(chalk.red(err.message));
    process.exit(1);
  }
}

async function listConfigs() {
  const spinner = ora("Fetching configs...").start();

  try {
    const response = await fetch(`${API_URL}/configs`);
    const configs = await response.json();

    spinner.stop();

    console.log(chalk.blue("Available Configs"));
    console.log(chalk.gray("─".repeat(40)));

    for (const config of configs) {
      console.log(`${chalk.cyan(config.name)}`);
      if (config.description) {
        console.log(chalk.gray(`  ${config.description}`));
      }
    }
  } catch (err) {
    spinner.fail("Failed to fetch configs");
    console.error(chalk.red(err.message));
    if (err.cause?.code === "ECONNREFUSED") {
      console.log(chalk.yellow("\nIs the server running? Try: make run"));
    }
    process.exit(1);
  }
}

function listTestDocs() {
  console.log(chalk.blue(`Test Documents (${TEST_DOCS_DIR})`));
  console.log(chalk.gray("─".repeat(40)));

  if (!fs.existsSync(TEST_DOCS_DIR)) {
    console.log(chalk.gray("No test-docs folder found"));
    return;
  }

  const files = fs.readdirSync(TEST_DOCS_DIR).filter((f) => {
    const ext = path.extname(f).toLowerCase();
    return [".pdf", ".png", ".jpg", ".jpeg", ".tiff"].includes(ext);
  });

  if (files.length === 0) {
    console.log(chalk.gray("No documents found"));
    console.log(chalk.gray("Place PDF or image files in test-docs/"));
    return;
  }

  for (const file of files) {
    const stats = fs.statSync(path.join(TEST_DOCS_DIR, file));
    const size = formatBytes(stats.size);
    console.log(`${chalk.white(file)} ${chalk.gray(size)}`);
  }
}

function statusColor(status) {
  switch (status) {
    case "completed":
      return chalk.green(status);
    case "failed":
      return chalk.red(status);
    case "processing":
      return chalk.yellow(status);
    default:
      return status;
  }
}

function formatBytes(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / (1024 * 1024)).toFixed(1) + " MB";
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
