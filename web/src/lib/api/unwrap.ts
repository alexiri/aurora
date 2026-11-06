import { ApiError } from "./client";

interface FetchResult<T> {
  data?: T;
  error?: unknown;
  response: Response;
}

/** Turn an openapi-fetch result into data, or throw an `ApiError`. */
export async function unwrap<T>(promise: Promise<FetchResult<T>>): Promise<T> {
  const { data, error, response } = await promise;
  if (error !== undefined || !response.ok) {
    throw ApiError.fromBody(error, response.status);
  }
  return data as T;
}
