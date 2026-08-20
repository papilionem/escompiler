// @expected-stdout-begin
// true
// true
// true
// @expected-stdout-end
try { null.x; } catch(e) { console.log(e instanceof TypeError); }
try { [].length = -1; } catch(e) { console.log(e instanceof RangeError); }
try { undeclared; } catch(e) { console.log(e instanceof ReferenceError); }
