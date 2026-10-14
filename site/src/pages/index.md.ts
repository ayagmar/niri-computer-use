import type { APIRoute } from 'astro';
import { homeMarkdown, text } from '../lib/markdown';

export const GET: APIRoute = () => text(homeMarkdown());
