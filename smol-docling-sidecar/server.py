"""
SmolDocling Sidecar Service

FastAPI service that wraps SmolDocling (256M VLM on MLX) for PDF processing.
Keeps model loaded in memory for fast per-request processing.
Same API contract as the docling sidecar.
"""

import logging
import os
import tempfile
import time
from typing import Any

from fastapi import FastAPI, File, UploadFile, HTTPException
from pydantic import BaseModel

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(
    title="SmolDocling Sidecar",
    description="PDF processing service using SmolDocling (MLX)",
    version="0.1.0",
)

# Lazy-loaded singletons
_model = None
_tokenizer = None
_model_config = None

MODEL_NAME = "ds4sd/SmolDocling-256M-preview-mlx-bf16"


def get_model():
    """Get or create the SmolDocling model (singleton)."""
    global _model, _tokenizer, _model_config
    if _model is None:
        logger.info("Loading SmolDocling model (first request)...")
        t0 = time.time()
        from mlx_vlm import load
        from mlx_vlm.utils import get_model_path, load_config

        _model, _tokenizer = load(MODEL_NAME)

        # Load model config for apply_chat_template
        model_path = get_model_path(MODEL_NAME)
        _model_config = load_config(model_path)

        logger.info(f"SmolDocling model loaded in {time.time() - t0:.1f}s")
    return _model, _tokenizer, _model_config


class PageContent(BaseModel):
    """OCR content for a single page."""
    page_num: int
    text: str


class ConversionResult(BaseModel):
    """Result of document conversion."""
    markdown: str
    pages: list[PageContent]
    total_pages: int
    metadata: dict[str, Any]


@app.get("/health")
async def health():
    """Health check endpoint."""
    return {"status": "ok"}


@app.post("/convert", response_model=ConversionResult)
async def convert_document(file: UploadFile = File(...)):
    """
    Convert a PDF document to structured output using SmolDocling VLM.

    Returns:
    - Full markdown export
    - Page-by-page OCR text
    - Document metadata
    """
    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")

    content = await file.read()
    logger.info(f"Received file: {file.filename} ({len(content)} bytes)")

    try:
        from pdf2image import convert_from_bytes
        from mlx_vlm import apply_chat_template, stream_generate
        from docling_core.types.doc.document import DocTagsDocument, DoclingDocument

        model, tokenizer, model_config = get_model()

        t0 = time.time()

        # Convert PDF pages to images
        logger.info("Converting PDF pages to images...")
        images = convert_from_bytes(content, dpi=150)
        num_pages = len(images)
        logger.info(f"Got {num_pages} pages in {time.time() - t0:.1f}s")

        # Process each page with SmolDocling
        prompt = "Convert this page to docling."
        page_doctags = []
        page_images = []

        # Create temp dir for page images (mlx_vlm 0.1.x needs file paths)
        with tempfile.TemporaryDirectory() as tmpdir:
            for i, image in enumerate(images):
                page_num = i + 1
                logger.info(f"Processing page {page_num}/{num_pages}...")
                t_page = time.time()

                # Save page image to temp file
                img_path = os.path.join(tmpdir, f"page_{page_num}.png")
                image.save(img_path, "PNG")

                # Format prompt with chat template
                formatted = apply_chat_template(
                    tokenizer, model_config, prompt
                )

                # Stream-generate doctags for this page
                output = ""
                for result in stream_generate(
                    model,
                    tokenizer,
                    formatted,
                    img_path,
                    max_tokens=8192,
                ):
                    output += result.text if hasattr(result, "text") else str(result)

                # Clean up output
                if output.endswith("<end_of_utterance>"):
                    output = output[: -len("<end_of_utterance>")]
                output = output.strip()

                page_doctags.append(output)
                page_images.append(image)
                logger.info(
                    f"Page {page_num} done in {time.time() - t_page:.1f}s "
                    f"({len(output)} chars doctags)"
                )

        # Build DoclingDocument from doctags + images
        doctags_doc = DocTagsDocument.from_doctags_and_image_pairs(
            page_doctags, page_images
        )
        docling_doc = DoclingDocument.load_from_doctags(doctags_doc)

        # Export full markdown
        markdown = docling_doc.export_to_markdown()

        # Export to dict for page-level text
        doc_dict = docling_doc.export_to_dict()

        pages_dict: dict[int, list[str]] = {i: [] for i in range(1, num_pages + 1)}

        texts = doc_dict.get("texts", [])
        for text_item in texts:
            text = text_item.get("text", "")
            if not text:
                continue
            prov = text_item.get("prov", [])
            page_no = 1
            if prov and len(prov) > 0:
                page_no = prov[0].get("page_no", 1)
            if page_no in pages_dict:
                pages_dict[page_no].append(text)

        pages = [
            PageContent(
                page_num=i,
                text="\n\n".join(pages_dict.get(i, [])),
            )
            for i in range(1, num_pages + 1)
        ]

        elapsed = time.time() - t0
        non_empty = sum(1 for p in pages if p.text)
        logger.info(
            f"Conversion complete: {num_pages} pages ({non_empty} with content), "
            f"{len(markdown)} chars markdown, {elapsed:.1f}s total"
        )

        return ConversionResult(
            markdown=markdown,
            pages=pages,
            total_pages=num_pages,
            metadata={
                "model": MODEL_NAME,
                "processing_time_s": round(elapsed, 2),
            },
        )

    except Exception as e:
        logger.exception(f"Conversion failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "3005"))
    uvicorn.run(app, host="0.0.0.0", port=port)
