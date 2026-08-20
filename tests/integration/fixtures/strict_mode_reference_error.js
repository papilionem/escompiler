// @expected-stdout: caught ReferenceError
"use strict";
try {
    undeclaredVar = 42;
} catch (e) {
    console.log("caught ReferenceError");
}
