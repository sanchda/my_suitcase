" Vim syntax file
" Language: k3, e333j
" Maintainer: ds,vj
" Last Change: 2018 January
" TODO operators with adverbs
" TODO \d: match which ignores a0:1
" TODO dots preceding words
" TODO jtype
" TODO system callbacks
" k system stuff
syn keyword kSystemFunction _log _exp _abs _sqr _sqrt _floor _sin _cos _tan _asin _acos _atan _sinh _cosh _tanh _lsq _draw _ci _ic _sv _vs _ss _sm _jd _dj _lt _gtime _ltime _lin _in _di _dvl _dv _bin _binl _inv _dot _mul _tl _ssr _hash _bd _db _getenv _host _exit _div _at _setenv _size _and _or _xor _rot _shift _not _del _rename _Free _addr _obj _phr _kv _vk _len _val _sleep _signal _kill _peach _csm _cz _zc _zd _dz _encrypt _decrypt _md5 _sha2 _hd _ceil _gamma _lgamma _erf _erfc _euclid _rint _trunc _choose _round _neuter _anoint _safe _netstat _install
syn keyword kConstant _a _b _c _d _f _h _i _k _m _n _o _p _r _s _t _u _v _w _y _P _T
syn keyword kConditional if
syn keyword kRepeat  while do
syn keyword kTodo  contained TODO FIXME XXX HACK
" Symbol matching, complete with ridiculous logic that allows for \" inside of quoted symbols
syntax match kTokens display "`\.\=\h\(\w\|\.\)*"
syntax match kTokens display "\v`\".{-}\\@<!\""
syn match kWords display "\.\=\h\(\w\|\.\)*"
syn match kColons display "\d\:"
" String and Character constants
" Highlight special characters (those which have a backslash) differently
syn match kSpecial display contained "\\\(x\x\+\|\o\{1,3}\|.\|$\)"
syn match kSpecial display contained "\\\(u\x\{4}\|U\x\{8}\)"
syn region kString  start=+L\="+ skip=+\\\\\|\\"+ end=+"+ contains=kSpecial,@Spell
syn match kNumbers display transparent "\<\d\|\.\d" contains=kNumber,kFloat
" Same, but without octal error (for comments)
syn match kNumbersCom display contained transparent "\<\d\|\.\d" contains=kNumber,kFloat
syn match kNumber  display contained "\d\+\(u\=l\{0,2}\|ll\=u\)\>"
syn match kFloat  display contained "\d\+f"
"floating point number, with dot, optional exponent
syn match kFloat  display contained "\d\+\.\d*\(e[-+]\=\d\+\)\=[fl]\="
"floating point number, starting with a dot, optional exponent
syn match kFloat  display contained "\.\d\+\(e[-+]\=\d\+\)\=[fl]\=\>"
"floating point number, without dot, with exponent
syn match kFloat  display contained "\d\+e[-+]\=\d\+[fl]\=\>"

syn match kOperator display "[\+\-\_\,\.\?\!\@\#\$\%\^\&\*\~\=\|]"
syn match kOperator  /[/\']/
syn match kOperator  /[/\']:/
syntax region kSlash start="^\\" end=/$/ contains=kComment
syntax region kComment start="^/" end="$" contains=kTodo
syntax region kComment start="\s/" end="$" contains=kTodo


syn match kSeparator ";"

"catch errrors
syn match kInvalidFunction "\<_[a-zA-Z][a-zA-Z0-9]*\>"
syn region kBlock       transparent start='\[' end='\]' contains=ALLBUT,kBlockError
syn match  kBlockError  "\]"
syn region kSet transparent start='{' end='}' contains=ALLBUT,kSetError
syn match  kSetError    "}"

" Define the default highlighting.
" Only used when an item doesn't have highlighting yet
hi def link kTokens  Identifier
hi def link kNumber Number
hi def link kFloat  Number
hi def link kConstant  Structure
hi def link kSystemFunction Structure
hi def link kColons  Error
hi def link kSpecial Special
"hi def link kWords  Identifier
hi def link kConditional Conditional
hi def link kRepeat  Repeat
hi def link kOperator  Operator
hi def link kSlash  PreProc
hi def link kString  String
hi def link kComment  Comment
hi def link kTodo  Todo
hi link kSeparator Type
  hi link kInvalidFunction      kError
  hi link kBlockError           kError
  hi link kSetError             kError
  hi link kError                Error
  hi link kWords Normal

let b:current_syntax = "k"
" vim: ts=2

