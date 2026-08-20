// @expected-stdout-begin
// caught undefined
// caught null
// caught 42
// caught hello
// caught true
// @expected-stdout-end
try { throw undefined; } catch (e) { console.log("caught", e); }
try { throw null; } catch (e) { console.log("caught", e); }
try { throw 42; } catch (e) { console.log("caught", e); }
try { throw "hello"; } catch (e) { console.log("caught", e); }
try { throw true; } catch (e) { console.log("caught", e); }
