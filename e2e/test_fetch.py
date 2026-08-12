async def test_fetch_returns_json(client, stub_server):
    result = await client.call_tool("fetch", {"url": stub_server})
    assert result.content[0].meta["dev.actcore/mime-type"] == "application/json"
    # Bonus: fetch also reports the upstream status via metadata (src/lib.rs
    # sends it as `http-client:status`); the old suite never checked it.
    assert result.content[0].meta["http-client:status"] == 200


async def test_fetch_sends_custom_headers(client, stub_server):
    result = await client.call_tool("fetch", {"url": stub_server, "headers": {"X-Test": "hello"}})
    assert result.content[0].meta["dev.actcore/mime-type"] == "application/json"
    # Bonus: the old assertion (mime_type == application/json, identical to
    # the plain-GET case above) never actually proved the header reached the
    # server — both requests get the same response either way. The stub
    # echoes request headers back, so this can now check the thing the test
    # name claims to check.
    echoed = result.content[0].text
    assert '"x-test": "hello"' in echoed


async def test_fetch_rejects_a_url_with_no_scheme(client, expect_error):
    await expect_error(
        client, "fetch", {"url": "not-a-url"}, "std:invalid-args",
        contains="Missing URL scheme",
    )
