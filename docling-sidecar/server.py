"""
Docling Sidecar Service

FastAPI service that wraps Docling for PDF processing.
Keeps models loaded in memory for fast per-request processing.
"""

import asyncio
import logging
import os
import pathlib
import signal
import tempfile
import threading
import time
import uuid
from typing import Any

from fastapi import FastAPI, File, UploadFile, HTTPException, Query
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

# Concurrency limit — prevents duplicate/parallel jobs from grinding the CPU
MAX_CONCURRENT = int(os.environ.get("DOCLING_MAX_CONCURRENT", "1"))
_conversion_sem = asyncio.Semaphore(MAX_CONCURRENT)

# Track threads running conversions (job_id -> thread ident)
_active_threads: dict[str, int] = {}

# Activity tracking for idle-shutdown (GCE auto-stop)
ACTIVITY_FILE = pathlib.Path("/tmp/docling_last_activity")


def touch_activity():
    try:
        ACTIVITY_FILE.touch()
    except OSError:
        pass


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


class JobProgress(BaseModel):
    """Progress of an active conversion job."""
    job_id: str
    filename: str
    status: str  # "loading_model" | "converting" | "exporting" | "completed" | "failed"
    pages_done: int
    total_pages: int
    elapsed_s: float


# Active job progress tracking — keyed by job_id
_jobs: dict[str, dict] = {}


@app.get("/health")
async def health():
    """Health check endpoint."""
    # Clean up finished jobs older than 120s
    now = time.time()
    stale = [
        jid for jid, j in _jobs.items()
        if j.get("completed_at") and now - j["completed_at"] > 120
    ]
    for jid in stale:
        del _jobs[jid]
    return {"status": "ok"}


@app.get("/progress", response_model=list[JobProgress])
async def list_progress():
    """List progress of all active jobs."""
    now = time.time()
    return [
        JobProgress(
            job_id=jid,
            filename=j["filename"],
            status=j["status"],
            pages_done=j["pages_done"],
            total_pages=j["total_pages"],
            elapsed_s=round(now - j["started_at"], 1),
        )
        for jid, j in _jobs.items()
    ]


@app.get("/progress/{job_id}", response_model=JobProgress)
async def get_progress(job_id: str):
    """Get progress of a specific job."""
    j = _jobs.get(job_id)
    if not j:
        raise HTTPException(status_code=404, detail="Job not found")
    return JobProgress(
        job_id=job_id,
        filename=j["filename"],
        status=j["status"],
        pages_done=j["pages_done"],
        total_pages=j["total_pages"],
        elapsed_s=round(time.time() - j["started_at"], 1),
    )


@app.post("/cancel/{job_id}")
async def cancel_job(job_id: str):
    """Cancel a specific running conversion. Best-effort thread kill."""
    tid = _active_threads.get(job_id)
    if not tid:
        j = _jobs.get(job_id)
        if j and j.get("status") in ("completed", "failed"):
            return {"status": "already_finished", "job_id": job_id}
        raise HTTPException(status_code=404, detail="Job not found or not running")

    import ctypes
    ctypes.pythonapi.PyThreadState_SetAsyncExc(
        ctypes.c_ulong(tid), ctypes.py_object(SystemExit)
    )
    _active_threads.pop(job_id, None)
    if job_id in _jobs:
        _jobs[job_id]["status"] = "cancelled"
    logger.info(f"Cancelled job {job_id} (thread {tid})")
    return {"status": "cancelled", "job_id": job_id}


@app.post("/cancel-all")
async def cancel_all():
    """Kill ALL running conversions by restarting the sidecar process.

    This is the nuclear option — converter.convert() is a blocking C call
    that can't be interrupted from Python. The startup wrapper auto-restarts us.
    """
    cancelled = list(_active_threads.keys())
    logger.warning(f"cancel-all: killing {len(cancelled)} jobs, restarting process")

    # Schedule hard exit after giving time for the HTTP response
    async def _exit():
        await asyncio.sleep(0.3)
        os._exit(0)

    asyncio.ensure_future(_exit())
    return {"status": "restarting", "cancelled": cancelled}


@app.post("/convert", response_model=ConversionResult)
async def convert_document(
    file: UploadFile = File(...),
    job_id: str = Query(default=None, description="Optional job ID for progress tracking"),
):
    """
    Convert a document to structured output using Docling.

    Returns:
    - Full markdown export
    - Page-by-page OCR text
    - Document metadata
    """
    touch_activity()

    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")

    content = await file.read()
    logger.info(f"Received file: {file.filename} ({len(content)} bytes)")

    jid = job_id or str(uuid.uuid4())
    _jobs[jid] = {
        "filename": file.filename,
        "status": "loading_model",
        "pages_done": 0,
        "total_pages": 0,
        "started_at": time.time(),
    }

    try:
        async with _conversion_sem:
            result = await asyncio.to_thread(_convert_blocking, content, file.filename, jid)
            return result
    except HTTPException:
        raise
    except Exception as e:
        _jobs[jid]["status"] = "failed"
        logger.exception(f"Conversion failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))


def _convert_blocking(content: bytes, filename: str, jid: str) -> ConversionResult:
    """Run the Docling pipeline in a worker thread so the event loop stays free."""
    import os

    _active_threads[jid] = threading.current_thread().ident
    converter = get_converter()
    _jobs[jid]["status"] = "converting"

    t0 = time.time()

    suffix = os.path.splitext(filename)[1] or ".pdf"
    with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
        tmp.write(content)
        tmp_path = tmp.name

    try:
        logger.info(f"Converting {filename}...")
        result = converter.convert(tmp_path)
        doc = result.document

        _jobs[jid]["status"] = "exporting"

        # Export to markdown
        markdown = doc.export_to_markdown()

        # Export to dict for page-level content
        doc_dict = doc.export_to_dict()

        # Get page count
        num_pages = len(doc_dict.get('pages', {}))
        if num_pages == 0:
            num_pages = 1  # fallback

        _jobs[jid]["total_pages"] = num_pages
        _jobs[jid]["pages_done"] = num_pages

        # Initialize pages dict
        pages_dict: dict[int, list[str]] = {i: [] for i in range(1, num_pages + 1)}

        # Extract text from texts array, grouped by page
        texts = doc_dict.get('texts', [])
        for text_item in texts:
            text = text_item.get('text', '')
            if not text:
                continue
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
                text="\n\n".join(pages_dict.get(i, [])),
            )
            for i in range(1, num_pages + 1)
        ]

        # Calculate stats
        elapsed = time.time() - t0
        non_empty_pages = sum(1 for p in pages if p.text)
        logger.info(
            f"Conversion complete: {num_pages} pages ({non_empty_pages} with content), "
            f"{len(markdown)} chars markdown, {elapsed:.1f}s total"
        )

        # Extract metadata
        metadata = doc_dict.get('origin', {})

        _jobs[jid]["status"] = "completed"
        _jobs[jid]["completed_at"] = time.time()

        return ConversionResult(
            markdown=markdown,
            pages=pages,
            total_pages=num_pages,
            metadata={
                **metadata,
                "processing_time_s": round(elapsed, 2),
                "job_id": jid,
            },
        )

    finally:
        _active_threads.pop(jid, None)
        os.unlink(tmp_path)


@app.post("/convert/json")
async def convert_document_json(file: UploadFile = File(...)):
    """
    Convert a document to Docling's native JSON format.

    Returns the full DoclingDocument as JSON for maximum detail.
    """
    touch_activity()

    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")

    content = await file.read()
    logger.info(f"Received file for JSON export: {file.filename} ({len(content)} bytes)")

    try:
        result = await asyncio.to_thread(_convert_json_blocking, content, file.filename)
        return JSONResponse(content=result)
    except Exception as e:
        logger.exception(f"JSON conversion failed: {e}")
        raise HTTPException(status_code=500, detail=str(e))


def _convert_json_blocking(content: bytes, filename: str) -> dict:
    """Run Docling JSON export in a worker thread."""
    import os

    converter = get_converter()

    suffix = os.path.splitext(filename)[1] or ".pdf"
    with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
        tmp.write(content)
        tmp_path = tmp.name

    try:
        result = converter.convert(tmp_path)
        doc = result.document
        json_output = doc.export_to_dict()
        logger.info(f"JSON export complete for {filename}")
        return json_output
    finally:
        os.unlink(tmp_path)


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "3001"))
    uvicorn.run(app, host="0.0.0.0", port=port)
