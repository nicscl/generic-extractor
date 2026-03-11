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
  .command("sections <file>")
  .description("Detect document sections with page ranges (lightweight, no full extraction)")
  .option("-c, --config <name>", "Extraction config to use", "legal_br")
  .option("--ocr <provider>", "OCR provider: gemini, docling, mistral_ocr", "gemini")
  .option("-o, --output <file>", "Output file path")
  .option("--md", "Also save markdown version")
  .option("--page", "Page-by-page mode (finer granularity, uses Gemini)")
  .option("--model <name>", "Model for page mode", "gemini-2.5-flash-lite-preview")
  .action(async (file, options) => {
    await detectSections(file, options);
  });

program
  .command("pages <file>")
  .description("Extract each page separately and save to a folder")
  .option("-c, --config <name>", "Extraction config to use", "legal_br")
  .option("--ocr <provider>", "OCR provider: gemini, gemini_pages, docling", "gemini_pages")
  .option("-o, --output <dir>", "Output directory")
  .option("--json", "Also save JSON version of each page")
  .action(async (file, options) => {
    await extractPages(file, options);
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

async function extractPages(file, options) {
  // Resolve file path
  let filePath = file;
  if (!path.isAbsolute(file)) {
    const testDocPath = path.join(TEST_DOCS_DIR, file);
    if (fs.existsSync(testDocPath)) {
      filePath = testDocPath;
    } else if (!fs.existsSync(file)) {
      console.error(chalk.red(`File not found: ${file}`));
      process.exit(1);
    }
  }

  if (!fs.existsSync(filePath)) {
    console.error(chalk.red(`File not found: ${filePath}`));
    process.exit(1);
  }

  console.log(chalk.blue("Page-by-Page Extraction"));
  console.log(chalk.gray("─".repeat(40)));
  console.log(`File:  ${chalk.white(path.basename(filePath))}`);
  console.log(`OCR:   ${chalk.white(options.ocr)}`);
  console.log(`API:   ${chalk.gray(API_URL)}`);
  console.log();

  const spinner = ora("Extracting pages...").start();

  try {
    const form = new FormData();
    form.append("file", fs.createReadStream(filePath));

    const ocrProvider = options.ocr === "gemini" ? "gemini_ocr" :
                        options.ocr === "gemini_pages" ? "gemini_ocr_pages" : options.ocr;
    const url = `${API_URL}/pages?config=${options.config}&ocr_provider=${ocrProvider}`;
    const response = await fetch(url, {
      method: "POST",
      body: form,
    });

    if (!response.ok) {
      const error = await response.text();
      spinner.fail("Page extraction failed");
      console.error(chalk.red(error));
      process.exit(1);
    }

    const result = await response.json();
    spinner.succeed(`Extracted ${result.total_pages} pages`);

    // Create output directory for this document
    const basename = path.basename(filePath, path.extname(filePath));
    const outDir = options.output || path.join(OUTPUT_DIR, basename);

    if (!fs.existsSync(outDir)) {
      fs.mkdirSync(outDir, { recursive: true });
    }

    // Save each page
    for (const page of result.pages || []) {
      const pageNum = String(page.page_num).padStart(2, "0");
      const mdFile = path.join(outDir, `page_${pageNum}.md`);

      // Add page header to markdown
      const pageContent = `# Page ${page.page_num}\n\n${page.text}`;
      fs.writeFileSync(mdFile, pageContent);

      if (options.json) {
        const jsonFile = path.join(outDir, `page_${pageNum}.json`);
        fs.writeFileSync(jsonFile, JSON.stringify(page, null, 2));
      }
    }

    // Save metadata
    const metaFile = path.join(outDir, "_metadata.json");
    fs.writeFileSync(metaFile, JSON.stringify({
      filename: result.filename,
      total_pages: result.total_pages,
      ocr_metadata: result.metadata,
      extracted_at: new Date().toISOString()
    }, null, 2));

    console.log(chalk.green(`\nSaved to: ${outDir}/`));
    console.log(chalk.gray(`  ${result.total_pages} page files (page_01.md, page_02.md, ...)`));
    console.log(chalk.gray(`  _metadata.json`));

    // Display OCR metadata if present
    if (result.metadata) {
      const ocr = result.metadata;
      console.log(chalk.blue("\nOCR Info"));
      console.log(chalk.gray("─".repeat(40)));
      if (ocr.provider) {
        console.log(`${chalk.cyan("Provider")}: ${ocr.provider} (${ocr.model || "?"})`);
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

  } catch (err) {
    spinner.fail("Request failed");
    console.error(chalk.red(err.message));
    if (err.cause?.code === "ECONNREFUSED") {
      console.log(chalk.yellow("\nIs the server running? Try: make run"));
    }
    process.exit(1);
  }
}

async function detectSections(file, options) {
  // Resolve file path
  let filePath = file;
  if (!path.isAbsolute(file)) {
    const testDocPath = path.join(TEST_DOCS_DIR, file);
    if (fs.existsSync(testDocPath)) {
      filePath = testDocPath;
    } else if (!fs.existsSync(file)) {
      console.error(chalk.red(`File not found: ${file}`));
      process.exit(1);
    }
  }

  if (!fs.existsSync(filePath)) {
    console.error(chalk.red(`File not found: ${filePath}`));
    process.exit(1);
  }

  const mode = options.page ? "page" : "full";

  console.log(chalk.blue("Document Section Detection"));
  console.log(chalk.gray("─".repeat(40)));
  console.log(`File:  ${chalk.white(path.basename(filePath))}`);
  console.log(`OCR:   ${chalk.white(options.ocr)}`);
  console.log(`Mode:  ${chalk.white(mode)}${options.page ? chalk.gray(` (${options.model})`) : ""}`);
  console.log(`API:   ${chalk.gray(API_URL)}`);
  console.log();

  const spinner = ora(options.page ? "Detecting sections page-by-page..." : "Detecting sections...").start();

  try {
    const form = new FormData();
    form.append("file", fs.createReadStream(filePath));

    const ocrProvider = options.ocr === "gemini" ? "gemini_ocr" : options.ocr;
    let url = `${API_URL}/sections?config=${options.config}&ocr_provider=${ocrProvider}&format=markdown&mode=${mode}`;
    if (options.page && options.model) {
      url += `&model=${encodeURIComponent(options.model)}`;
    }
    const response = await fetch(url, {
      method: "POST",
      body: form,
    });

    if (!response.ok) {
      const error = await response.text();
      spinner.fail("Section detection failed");
      console.error(chalk.red(error));
      process.exit(1);
    }

    const result = await response.json();
    spinner.succeed(`Detected ${result.sections?.length || 0} sections`);

    // Ensure output directory exists
    if (!fs.existsSync(OUTPUT_DIR)) {
      fs.mkdirSync(OUTPUT_DIR, { recursive: true });
    }

    // Generate output filename
    const basename = path.basename(filePath, path.extname(filePath));
    const timestamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
    const outFile = options.output || path.join(OUTPUT_DIR, `${basename}_sections_${timestamp}.json`);

    // Save JSON result
    fs.writeFileSync(outFile, JSON.stringify(result, null, 2));
    console.log(chalk.green(`\nSaved: ${outFile}`));

    // Save markdown if requested
    if (options.md && result.markdown) {
      const mdFile = outFile.replace(/\.json$/, ".md");
      fs.writeFileSync(mdFile, result.markdown);
      console.log(chalk.green(`Markdown: ${mdFile}`));
    }

    // Display sections table
    console.log(chalk.blue("\nDocument Sections"));
    console.log(chalk.gray("─".repeat(50)));
    console.log(chalk.gray("Section".padEnd(35) + "Type".padEnd(15) + "Pages"));
    console.log(chalk.gray("─".repeat(50)));

    for (const section of result.sections || []) {
      const name = (section.name || "").substring(0, 33).padEnd(35);
      const type = (section.section_type || "-").substring(0, 13).padEnd(15);
      const pages = section.page_start === section.page_end
        ? `p${section.page_start}`
        : `p${section.page_start}-${section.page_end}`;
      console.log(`${chalk.white(name)}${chalk.cyan(type)}${chalk.yellow(pages)}`);
    }

    // Display OCR metadata if present
    if (result.ocr_metadata) {
      const ocr = result.ocr_metadata;
      console.log(chalk.blue("\nOCR Info"));
      console.log(chalk.gray("─".repeat(40)));
      if (ocr.provider) {
        console.log(`${chalk.cyan("Provider")}: ${ocr.provider} (${ocr.model || "?"})`);
      }
      if (ocr.cost_usd !== undefined) {
        console.log(`${chalk.cyan("Cost")}: $${ocr.cost_usd.toFixed(4)}`);
      }
    }

  } catch (err) {
    spinner.fail("Request failed");
    console.error(chalk.red(err.message));
    if (err.cause?.code === "ECONNREFUSED") {
      console.log(chalk.yellow("\nIs the server running? Try: make run"));
    }
    process.exit(1);
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
