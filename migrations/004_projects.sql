-- Projects: organize extractions and datasets
CREATE TABLE extraction.projects (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT DEFAULT '',
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Link existing tables to projects
ALTER TABLE extraction.extractions
    ADD COLUMN project_id UUID REFERENCES extraction.projects(id) ON DELETE SET NULL;
ALTER TABLE extraction.datasets
    ADD COLUMN project_id UUID REFERENCES extraction.projects(id) ON DELETE SET NULL;

-- Chat history per project
CREATE TABLE extraction.chat_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES extraction.projects(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL,
    tool_calls JSONB,
    tool_call_id TEXT,
    tool_name TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Indexes
CREATE INDEX idx_projects_user ON extraction.projects(user_id);
CREATE INDEX idx_extractions_project ON extraction.extractions(project_id);
CREATE INDEX idx_datasets_project ON extraction.datasets(project_id);
CREATE INDEX idx_chat_messages_project ON extraction.chat_messages(project_id, created_at);

-- RLS
ALTER TABLE extraction.projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE extraction.chat_messages ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users can CRUD own projects" ON extraction.projects
    FOR ALL USING (auth.uid() = user_id) WITH CHECK (auth.uid() = user_id);
CREATE POLICY "Users can CRUD own chat messages" ON extraction.chat_messages
    FOR ALL USING (project_id IN (SELECT id FROM extraction.projects WHERE user_id = auth.uid()))
    WITH CHECK (project_id IN (SELECT id FROM extraction.projects WHERE user_id = auth.uid()));
