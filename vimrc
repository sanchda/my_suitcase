" === Global Settings ===
set modeline
set encoding=utf-8
set ignorecase
set smartcase
set foldenable
set foldmethod=indent 
set t_Co=256
set backspace=indent,eol,start 
set incsearch
set lazyredraw
set showmatch
set showcmd
set hlsearch
set nu
set ruler
set autoread
set nobackup
set noerrorbells
set novisualbell
set t_vb=
set background=dark
set tm=500
let $MYVIMRC="$SUITCASE/vim/vimrc"

" === Color column stuff
execute "set colorcolumn=" . join(range(81,335), ',')
highlight ColorColumn ctermbg=0

" === Syntax Highlighting & auto-indent ===
let python_highlight_all=1
syntax on
syntax enable
set ofu=syntaxcomplete#Complete
set antialias
filetype on
filetype plugin on 
filetype indent on
set autoindent
set smartindent

" === vim SLIME stuff ===
let g:slime_target = "tmux"
let g:slime_paste_file = "$HOME/.slime_paste"


" === Wildmenu ===
set wildmenu
set wildmode=longest,list,full
set wildignore=.svn,CVS,*.o,*.a,*.class,*.mo,*.la,*.so,*.obj,*.swp,*.jpg,*.png,*.xpm,*.gif


" === Other Settings ===
" Show < or > when characters are not displayed on the left or right.
set list
set list listchars=nbsp:¬,tab:>-,precedes:<,extends:>


" === Coding tweaks ===
fu Dave_style()
  set shiftwidth=2
  set expandtab
  set tabstop=2
  set softtabstop=2
  set shiftwidth=2
endf
au BufRead,BufNewFile *.sh,*.js,*.html,*.css,*py,*pyw,*.c,*.h,*.cpp,*.hpp call Dave_style()
au BufRead,BufNewFile Makefile* set noexpandtab

" TEX settings
au BufRead,BufNewFile *.tex call Dave_style()

" JL settings
au BufRead,BufNewFile *.jl set filetype=julia
au BufRead,BufNewFile *.jl call Dave_style()

" strace settings
au BufRead,BufNewFile *.strace set filetype=strace

" Use the below highlight group when displaying bad whitespace is desired.
highlight BadWhitespace ctermbg=red guibg=red
" Display tabs at the beginning of a line in Python mode as bad.
au BufRead,BufNewFile *.py,*.pyw match BadWhitespace /^\t\+/
au BufRead,BufNewFile *.k,*.py,*.pyw,*.c,*.h match BadWhitespace /\s\+$/
 
" Python: not needed, C: prevents insertion of '*' at the beginning of every line in a comment
au BufRead,BufNewFile *.c,*.h set formatoptions-=c formatoptions-=o formatoptions-=r
