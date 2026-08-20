// @expected-stdout-begin
// caught undefined
// caught null
// caught true
// caught false
// caught 42
// caught hello
// @expected-stdout-end
// Catching different value types
try { throw undefined; } catch(e) { console.log("caught", e); }
try { throw null; } catch(e) { console.log("caught", e); }
try { throw true; } catch(e) { console.log("caught", e); }
try { throw false; } catch(e) { console.log("caught", e); }
try { throw 42; } catch(e) { console.log("caught", e); }
try { throw "hello"; } catch(e) { console.log("caught", e); }
