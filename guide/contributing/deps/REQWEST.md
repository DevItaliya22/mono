# reqwest

## A HEAD object's size is the header, not the response body length

`reqwest::Response::content_length()` reports the known length of the response
body. It deliberately does not mean “parse the `Content-Length` header.” A HEAD
response has no body, so that method can report zero even when the server sends
`Content-Length: 473983562` to describe the selected object.

Code using HEAD to inspect an object's size must read
`response.headers()[CONTENT_LENGTH]` and parse that value. Keep absence distinct
from zero: no header means the object exists but its size is unknown; a literal
zero means the server explicitly described a zero-byte object.

This matters in `koru-release` model publication. The archive key includes its
digest, so the preflight refuses an existing object whose size differs from the
local artifact. Reading the response body length made every correctly uploaded
archive look truncated to zero bytes and blocked the next nightly.

Reference: [reqwest `Response::content_length`](https://docs.rs/reqwest/latest/reqwest/struct.Response.html#method.content_length).
