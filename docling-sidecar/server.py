"""
Docling Sidecar Service

FastAPI service that wraps Docling for PDF processing.
Keeps models loaded in memory for fast per-request processing.
"""

import io
import logging
from typing import Any

from fastapi import FastAPI, File, UploadFile, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(
    title="Docling Sidecar",
    description="PDF processing service using Docling",
    version="0.1.0",
)

# Lazy-load docling to avoid import time at startup
_converter = None

def get_converter():
    """Get or create the document converter (singleton)."""
    global _converter
    if _converter is None:
        logger.info("Loading Docling converter (first request)...")
        from docling.document_converter import DocumentConverter
        _converter = DocumentConverter()
        logger.info("Docling converter loaded!")
    return _converter


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
    Convert a PDF document to structured output.
    
    Returns:
    - Full markdown export
    - Page-by-page OCR text
    - Document metadata
    """
    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")
    
    # Read file content
    content = await file.read()
    logger.info(f"Received file: {file.filename} ({len(content)} bytes)")
    
    try:
        converter = get_converter()
        
        # Write to temp file (docling needs file path or URL)
        import tempfile
        import os
        
        suffix = os.path.splitext(file.filename)[1] or ".pdf"
        with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
            tmp.write(content)
            tmp_path = tmp.name
        
        try:
            # Convert document
            logger.info(f"Converting {file.filename}...")
            result = converter.convert(tmp_path)
            doc = result.document
            
            # Export to markdown
            markdown = doc.export_to_markdown()
            
            # Export to dict to reliably access page-level content
            doc_dict = doc.export_to_dict()
            
            # Get page count
            num_pages = len(doc_dict.get('pages', {}))
            if num_pages == 0:
                num_pages = 99  # fallback
            
            # Initialize pages dict
            pages_dict: dict[int, list[str]] = {i: [] for i in range(1, num_pages + 1)}
            
            # Extract text from texts array, grouped by page
            texts = doc_dict.get('texts', [])
            for text_item in texts:
                text = text_item.get('text', '')
                if not text:
                    continue
                
                # Get page number from prov
                prov = text_item.get('prov', [])
                page_no = 1
                if prov and len(prov) > 0:
                    page_no = prov[0].get('page_no', 1)
                
                if page_no in pages_dict:
                    pages_dict[page_no].append(text)
            
            # Build pages list
            pages = [
                PageContent(
                    page_num=i, 
                    text="\n\n".join(pages_dict.get(i, []))
                )
                for i in range(1, num_pages + 1)
            ]
            
            # Calculate stats
            non_empty_pages = sum(1 for p in pages if p.text)
            logger.info(f"Conversion complete: {num_pages} pages ({non_empty_pages} with content), {len(markdown)} chars markdown")
            
            # Extract metadata
            metadata = doc_dict.get('origin', {})
            
            return ConversionResult(
                markdown=markdown,
                pages=pages,
                total_pages=num_pages,
                metadata=metadata,
            )
            
        finally:
            # Clean up temp file
            os.unlink(tmp_path)
            
    except Exception as e:
        logger.exception(f"Conversion failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/convert/json")
async def convert_document_json(file: UploadFile = File(...)):
    """
    Convert a PDF document to Docling's native JSON format.
    
    Returns the full DoclingDocument as JSON for maximum detail.
    """
    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")
    
    content = await file.read()
    logger.info(f"Received file for JSON export: {file.filename} ({len(content)} bytes)")
    
    try:
        converter = get_converter()
        
        import tempfile
        import os
        
        suffix = os.path.splitext(file.filename)[1] or ".pdf"
        with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
            tmp.write(content)
            tmp_path = tmp.name
        
        try:
            result = converter.convert(tmp_path)
            doc = result.document
            
            # Export to JSON
            json_output = doc.export_to_dict()
            
            logger.info(f"JSON export complete for {file.filename}")
            return JSONResponse(content=json_output)
            
        finally:
            os.unlink(tmp_path)
            
    except Exception as e:
        logger.exception(f"JSON conversion failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=3001)
