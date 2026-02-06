-- Title      : STANDARD package
-- Library    : STD
-- Purpose    : This package defines the predefined types and subprograms 
--              implicit to every VHDL design unit.

package STANDARD is

    -- Predefined enumeration types
    type BOOLEAN is (FALSE, TRUE);
    type BIT is ('0', '1');
    type CHARACTER is (
        NUL, SOH, STX, ETX, EOT, ENQ, ACK, BEL,
        BS,  HT,  LF,  VT,  FF,  CR,  SO,  SI,
        DLE, DC1, DC2, DC3, DC4, NAK, SYN, ETB,
        CAN, EM,  SUB, ESC, FSP, GSP, RSP, USP,
        ' ', '!', '"', '#', '$', '%', '&', ''',
        '(', ')', '*', '+', ',', '-', '.', '/',
        '0', '1', '2', '3', '4', '5', '6', '7',
        '8', '9', ':', ';', '<', '=', '>', '?',
        '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G',
        'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
        'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W',
        'X', 'Y', 'Z', '[', '\', ']', '^', '_',
        '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
        'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
        'p', 'q', 'r', 's', 't', 'u', 'v', 'w',
        'x', 'y', 'z', '{', '|', '}', '~', DEL
    );

    type SEVERITY_LEVEL is (NOTE, WARNING, ERROR, FAILURE);

    -- Predefined numeric types
    -- Ranges are implementation dependent; these are typical 32-bit values
    type INTEGER is range -2147483648 to 2147483647;
    type REAL is range -1.0E38 to 1.0E38;

    -- Predefined physical types
    type TIME is range -2147483647 to 2147483647
    units
        fs;            -- femtosecond
        ps = 1000 fs;  -- picosecond
        ns = 1000 ps;  -- nanosecond
        us = 1000 ns;  -- microsecond
        ms = 1000 us;  -- millisecond
        sec = 1000 ms; -- second
        min = 60 sec;  -- minute
        hr = 60 min;   -- hour
    end units;

    -- Subtypes of INTEGER
    subtype NATURAL is INTEGER range 0 to INTEGER'HIGH;
    subtype POSITIVE is INTEGER range 1 to INTEGER'HIGH;

    -- Predefined array types
    type STRING is array (POSITIVE range <>) of CHARACTER;
    type BIT_VECTOR is array (NATURAL range <>) of BIT;

    -- Predefined functions
    function NOW return TIME;

    -- Note: Operators like "+", "-", "AND", "OR" are implicitly defined 
    -- for these types by the compiler and are not explicitly listed 
    -- as functions in the source file of this package.

end package STANDARD;
